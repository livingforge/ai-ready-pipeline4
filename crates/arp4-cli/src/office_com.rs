//! Late-bound COM automation of desktop Office, shared by the disposable worker
//! processes that render Excel ranges and remove rights management protection.
#![cfg(windows)]

use anyhow::{Context, Result, bail, ensure};
use std::{collections::HashSet, mem::ManuallyDrop};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            Com::{
                CLSCTX_LOCAL_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, CoCreateInstance,
                CoInitializeEx, CoUninitialize, DISPATCH_FLAGS, DISPATCH_METHOD,
                DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, IDispatch,
            },
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE, TerminateProcess},
            Variant::{
                VARIANT, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_ERROR, VT_I2, VT_I4, VT_INT, VT_R8,
                VariantClear,
            },
        },
    },
    core::{BSTR, GUID, PCWSTR},
};

pub(crate) struct Handle(pub(crate) HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) struct Apartment;
impl Apartment {
    pub(crate) fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

/// The IDs of the running processes whose executable is `name`.
pub(crate) fn processes(name: &str) -> Result<HashSet<u32>> {
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? });
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut ids = HashSet::new();
    unsafe { Process32FirstW(snapshot.0, &mut entry) }
        .with_context(|| format!("Cannot enumerate existing {name} processes"))?;
    loop {
        let end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case(name) {
            ids.insert(entry.th32ProcessID);
        }
        if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
            break;
        }
    }
    Ok(ids)
}

/// A job that terminates process `id` when the returned handle is dropped.
pub(crate) fn own_process(id: u32) -> Result<Handle> {
    let job = Handle(unsafe { CreateJobObjectW(None, PCWSTR::null())? });
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
        let process = Handle(OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE,
            false,
            id,
        )?);
        AssignProcessToJobObject(job.0, process.0).context("Cannot isolate the Office process")?;
    }
    Ok(job)
}

/// Terminates process `id`, which this worker created, when it cannot be owned.
pub(crate) fn terminate(id: u32) {
    if let Ok(process) = unsafe { OpenProcess(PROCESS_TERMINATE, false, id) } {
        let process = Handle(process);
        unsafe {
            let _ = TerminateProcess(process.0, 1);
        }
    }
}

/// A new instance of the Office application `prog_id` in a process of its own,
/// whose executable is `executable`, with the job that terminates it. An
/// application that attaches to a running instance is refused, so the user's
/// open documents are never touched.
pub(crate) fn isolated_application(prog_id: &str, executable: &str) -> Result<(IDispatch, Handle)> {
    let existing = processes(executable)?;
    let wide: Vec<u16> = prog_id.encode_utf16().chain(Some(0)).collect();
    let clsid = unsafe { CLSIDFromProgID(PCWSTR(wide.as_ptr())) }
        .with_context(|| format!("{prog_id} is not installed"))?;
    let application: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)? };
    let created: Vec<u32> = processes(executable)?
        .difference(&existing)
        .copied()
        .collect();
    let [id] = created[..] else {
        bail!(
            "{prog_id} did not start a process of its own; close the running {executable} and retry"
        );
    };
    match own_process(id) {
        Ok(job) => Ok((application, job)),
        Err(error) => {
            terminate(id);
            Err(error)
        }
    }
}

pub(crate) enum Arg<'a> {
    Int(i32),
    Float(f64),
    Bool(bool),
    Text(&'a str),
    Object(&'a IDispatch),
    Missing,
}

fn argument(arg: Arg<'_>) -> VARIANT {
    let mut value = VARIANT::default();
    let inner = unsafe { &mut value.Anonymous.Anonymous };
    match arg {
        Arg::Int(v) => {
            inner.vt = VT_I4;
            inner.Anonymous.lVal = v;
        }
        Arg::Float(v) => {
            inner.vt = VT_R8;
            inner.Anonymous.dblVal = v;
        }
        Arg::Bool(v) => {
            inner.vt = VT_BOOL;
            inner.Anonymous.boolVal.0 = if v { -1 } else { 0 };
        }
        Arg::Text(v) => {
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(v));
        }
        Arg::Object(v) => {
            inner.vt = VT_DISPATCH;
            inner.Anonymous.pdispVal = ManuallyDrop::new(Some(v.clone()));
        }
        Arg::Missing => {
            inner.vt = VT_ERROR;
            inner.Anonymous.scode = 0x8002_0004_u32 as i32;
        }
    }
    value
}

pub(crate) struct Value(VARIANT);
impl Drop for Value {
    fn drop(&mut self) {
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}
impl Value {
    fn inner(&self) -> &windows::Win32::System::Variant::VARIANT_0_0 {
        unsafe { &self.0.Anonymous.Anonymous }
    }
    pub(crate) fn dispatch(&self) -> Result<IDispatch> {
        let inner = self.inner();
        ensure!(
            inner.vt == VT_DISPATCH,
            "Office returned a non-object value"
        );
        unsafe { (*inner.Anonymous.pdispVal).clone() }.context("Office returned a null object")
    }
    pub(crate) fn int(&self) -> Result<i32> {
        let inner = self.inner();
        match inner.vt {
            VT_I4 | VT_INT => Ok(unsafe { inner.Anonymous.lVal }),
            VT_I2 => Ok(i32::from(unsafe { inner.Anonymous.iVal })),
            // Excel returns some enumerations, such as FileFormat, as doubles.
            VT_R8 if unsafe { inner.Anonymous.dblVal }.fract() == 0.0 => {
                Ok(unsafe { inner.Anonymous.dblVal } as i32)
            }
            vt => bail!("Office returned a non-integer value (VARTYPE {})", vt.0),
        }
    }
    pub(crate) fn float(&self) -> Result<f64> {
        let inner = self.inner();
        match inner.vt {
            VT_R8 => Ok(unsafe { inner.Anonymous.dblVal }),
            VT_I4 => Ok(unsafe { inner.Anonymous.lVal } as f64),
            _ => bail!("Office returned a non-numeric value"),
        }
    }
    pub(crate) fn string(&self) -> Result<String> {
        let inner = self.inner();
        ensure!(inner.vt == VT_BSTR, "Office returned a non-string value");
        Ok(unsafe { (*inner.Anonymous.bstrVal).to_string() })
    }
    pub(crate) fn boolean(&self) -> Result<bool> {
        let inner = self.inner();
        ensure!(inner.vt == VT_BOOL, "Office returned a non-boolean value");
        Ok(unsafe { inner.Anonymous.boolVal.0 != 0 })
    }
}

pub(crate) fn invoke(
    object: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    args: Vec<Arg<'_>>,
) -> Result<Value> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let member = PCWSTR(wide.as_ptr());
    let mut id = 0;
    unsafe { object.GetIDsOfNames(&GUID::zeroed(), &member, 1, 0, &mut id) }
        .with_context(|| format!("Office member {name:?} was not found"))?;
    let mut values: Vec<VARIANT> = args.into_iter().rev().map(argument).collect();
    let mut property_id = -3;
    let mut params = DISPPARAMS {
        rgvarg: values.as_mut_ptr(),
        rgdispidNamedArgs: std::ptr::null_mut(),
        cArgs: values.len() as u32,
        cNamedArgs: 0,
    };
    if flags == DISPATCH_PROPERTYPUT {
        params.rgdispidNamedArgs = &mut property_id;
        params.cNamedArgs = 1;
    }
    let mut result = VARIANT::default();
    let status = unsafe {
        object.Invoke(
            id,
            &GUID::zeroed(),
            0,
            flags,
            &params,
            Some(&mut result),
            None,
            None,
        )
    };
    for value in &mut values {
        unsafe {
            let _ = VariantClear(value);
        }
    }
    let result = Value(result);
    status.with_context(|| format!("Office COM call {name:?} failed"))?;
    Ok(result)
}

pub(crate) fn get(object: &IDispatch, name: &str) -> Result<Value> {
    invoke(object, name, DISPATCH_PROPERTYGET, vec![])
}
pub(crate) fn prop(object: &IDispatch, name: &str) -> Result<IDispatch> {
    get(object, name)?.dispatch()
}
pub(crate) fn put(object: &IDispatch, name: &str, arg: Arg<'_>) -> Result<()> {
    invoke(object, name, DISPATCH_PROPERTYPUT, vec![arg])?;
    Ok(())
}
pub(crate) fn call(object: &IDispatch, name: &str, args: Vec<Arg<'_>>) -> Result<Value> {
    invoke(object, name, DISPATCH_METHOD, args)
}
pub(crate) fn method(object: &IDispatch, name: &str, args: Vec<Arg<'_>>) -> Result<IDispatch> {
    call(object, name, args)?.dispatch()
}
pub(crate) fn item(object: &IDispatch, arg: Arg<'_>) -> Result<IDispatch> {
    invoke(object, "Item", DISPATCH_PROPERTYGET, vec![arg])?.dispatch()
}
