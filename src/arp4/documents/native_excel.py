"""Excel-owned structural edits: hidden, isolated instance, macros/events/links disabled.

Excel performs reference adjustments, table expansion and DrawingML serialization.
The original is opened read-only; SaveCopyAs writes only a staged output.
"""
from __future__ import annotations

import contextlib
import gc
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from zipfile import ZipFile

from . import excel, operations
from .contracts import DocumentError, digest, under, read, write

_OWNER_FILE: Path | None = None


@contextlib.contextmanager
def application():
    if os.name != "nt":
        raise DocumentError("structural Excel writeback requires Windows + Microsoft Excel + pip install ai-ready-pipeline4[writeback]")
    try:
        import pythoncom
        import win32com.client
        import win32api
        import win32process
    except ImportError as exc:
        raise DocumentError("install ai-ready-pipeline4[writeback] for structural Excel writeback") from exc
    pythoncom.CoInitialize()
    app = None
    auto_correct = None
    try:
        existing_processes = set(win32process.EnumProcesses())
        app = win32com.client.DispatchEx("Excel.Application")
        _, process_id = win32process.GetWindowThreadProcessId(app.Hwnd)
        if process_id in existing_processes:
            app = None  # Never modify or quit a pre-existing user's Excel instance.
            raise DocumentError("Excel did not create an isolated instance")
        if _OWNER_FILE is not None:
            handle = win32api.OpenProcess(0x0400, False, process_id)
            try:
                created = win32process.GetProcessTimes(handle)["CreationTime"].isoformat()
            finally:
                handle.Close()
            write(_OWNER_FILE, {"pid": process_id, "created": created})
        app.Visible = False
        app.DisplayAlerts = False
        app.EnableEvents = False
        app.AskToUpdateLinks = False
        app.AutomationSecurity = 3  # msoAutomationSecurityForceDisable
        auto_correct = (app.AutoCorrect.AutoExpandListRange, app.AutoCorrect.AutoFillFormulasInLists)
        app.AutoCorrect.AutoExpandListRange = False
        app.AutoCorrect.AutoFillFormulasInLists = False
        yield app
    except DocumentError:
        raise
    except Exception as exc:
        raise DocumentError(f"Excel automation failed: {exc}") from exc
    finally:
        if app is not None:
            try:
                if auto_correct is not None:
                    app.AutoCorrect.AutoExpandListRange, app.AutoCorrect.AutoFillFormulasInLists = auto_correct
                gc.collect()
                app.Quit()
            except Exception:
                pass
        app = None
        gc.collect()
        pythoncom.CoUninitialize()


def _open(app, path: Path):
    return app.Workbooks.Open(str(path.resolve()), UpdateLinks=0, ReadOnly=True,
                              IgnoreReadOnlyRecommended=True, AddToMru=False,
                              Password="", WriteResPassword="")


def _shape(sheet, name):
    try:
        return sheet.Shapes.Item(name)
    except Exception as exc:
        raise DocumentError(f"shape not found: {sheet.Name}/{name}") from exc


def _rgb(text: str) -> int:
    return int(text[1:3], 16) | int(text[3:5], 16) << 8 | int(text[5:7], 16) << 16


def _properties(shape, properties):
    if "text" in properties:
        shape.TextFrame2.TextRange.Text = properties["text"]
    if "font_size" in properties:
        shape.TextFrame2.TextRange.Font.Size = properties["font_size"]
    resize = any(key in properties for key in ("width", "height"))
    locked = shape.LockAspectRatio if resize else None
    try:
        if resize:
            shape.LockAspectRatio = 0
        for key in ("left", "top", "width", "height", "rotation"):
            if key in properties:
                setattr(shape, key.title(), properties[key])
    finally:
        if resize:
            shape.LockAspectRatio = locked
    for key in ("fill", "line"):
        if key in properties:
            component = getattr(shape, key.title())
            value = properties[key]
            component.Visible = 0 if value == "none" else -1
            if value != "none":
                if key == "fill":
                    component.Solid()
                component.ForeColor.RGB = _rgb(value)
    if "line_weight" in properties:
        shape.Line.Weight = properties["line_weight"]


def _verify_properties(shape, properties):
    for key, expected in properties.items():
        if key == "text":
            actual = shape.TextFrame2.TextRange.Text
            if actual.replace("\r\n", "\n").replace("\r", "\n") != expected.replace("\r\n", "\n").replace("\r", "\n"):
                raise DocumentError(f"shape text read-back failed: {shape.Name}")
        elif key in ("fill", "line"):
            component = getattr(shape, key.title())
            if expected == "none":
                valid = component.Visible == 0
            else:
                valid = component.Visible != 0 and component.ForeColor.RGB == _rgb(expected)
            if not valid:
                raise DocumentError(f"shape color read-back failed: {shape.Name}/{key}")
        else:
            actual = shape.Line.Weight if key == "line_weight" else shape.TextFrame2.TextRange.Font.Size if key == "font_size" else getattr(shape, key.title())
            if abs(float(actual) - expected) > 0.1:
                raise DocumentError(f"shape geometry read-back failed: {shape.Name}/{key}")


def _edit_shape(book, op, directory):
    sheet = book.Worksheets.Item(op["sheet"])
    kind, props = op["kind"], op.get("properties", {})
    if kind == "delete_shape":
        _shape(sheet, op["name"]).Delete()
        return
    if kind == "update_shape":
        shape = _shape(sheet, op["name"])
    elif kind == "add_shape":
        x, y, w, h = (props[k] for k in ("left", "top", "width", "height"))
        if op["shape_type"] == "textbox":
            shape = sheet.Shapes.AddTextbox(1, x, y, w, h)
        elif op["shape_type"] == "line":
            shape = sheet.Shapes.AddLine(x, y, x + w, y + h)
        else:
            shape = sheet.Shapes.AddShape({"rectangle": 1, "rounded_rectangle": 5, "ellipse": 9}[op["shape_type"]], x, y, w, h)
        shape.Name = op["name"]
    elif kind in ("add_picture", "replace_picture"):
        asset = under(directory, op["asset"])
        if digest(asset.read_bytes()) != op["asset_sha256"]:
            raise DocumentError("picture asset changed after validation")
        geometry = dict(props)
        attachments = []
        preserved = None
        if kind == "replace_picture":
            previous = _shape(sheet, op["name"])
            preserved = (previous.Placement, previous.ZOrderPosition, previous.AlternativeText, previous.LockAspectRatio)
            for i in range(1, sheet.Shapes.Count + 1):
                connector = sheet.Shapes.Item(i)
                if connector.Connector:
                    for side in ("Begin", "End"):
                        link = connector.ConnectorFormat
                        if getattr(link, side + "Connected") and getattr(link, side + "ConnectedShape").Name == op["name"]:
                            attachments.append((connector.Name, side, getattr(link, side + "ConnectionSite")))
            for key in ("left", "top", "width", "height", "rotation"):
                geometry.setdefault(key, getattr(previous, key.title()))
            previous.Delete()
        shape = sheet.Shapes.AddPicture(str(asset), False, True, *(geometry[k] for k in ("left", "top", "width", "height")))
        shape.Name = op["name"]
        props = geometry
        if preserved:
            shape.Placement, order, shape.AlternativeText, shape.LockAspectRatio = preserved
            while shape.ZOrderPosition > order:
                shape.ZOrder(3)  # msoSendBackward
            for name, side, site in attachments:
                getattr(_shape(sheet, name).ConnectorFormat, side + "Connect")(shape, site)
    elif kind == "add_connector":
        shape = sheet.Shapes.AddConnector({"straight": 1, "elbow": 2, "curve": 3}[op["connector_type"]], 0, 0, 100, 100)
        shape.Name = op["name"]
        begin = _shape(sheet, op["begin"]["name"])
        end = _shape(sheet, op["end"]["name"])
        shape.ConnectorFormat.BeginConnect(begin, op["begin"]["site"])
        shape.ConnectorFormat.EndConnect(end, op["end"]["site"])
        shape.RerouteConnections()
    else:
        raise DocumentError(f"unsupported shape operation: {kind}")
    _properties(shape, props)


def _verify_rows(source: Path, destination: Path, changes: list[dict], rows: list[dict]) -> None:
    """Independently re-read scalar cells that should survive the structural edit."""
    overwritten = {(c["sheet"], c["cell"]) for c in changes}
    actual = {(s["name"], c["address"]): c for s in excel.extract(destination) for c in s["cells"]}
    for sheet in excel.extract(source):
        for cell in sheet["cells"]:
            if cell["type"] in ("formula", "error"):
                continue
            column, row = operations.coordinate(cell["address"])
            final_row = operations.transform_row(sheet["name"], row, rows)
            if final_row is None:
                continue
            key = (sheet["name"], f"{column}{final_row}")
            if key in overwritten:
                continue
            found = actual.get(key, {"value": None})["value"]
            if found != cell["value"] and not (cell["value"] == "" and found is None):
                raise DocumentError(f"row movement read-back failed: {key}")


def _verify(book, changes, ops):
    for change in changes:
        cell = book.Worksheets.Item(change["sheet"]).Range(change["cell"])
        if change.get("kind") == "formula":
            actual = cell.Formula2 if cell.HasFormula else None
            if actual != change["after"]:
                raise DocumentError(f"formula read-back failed: {change['sheet']}!{change['cell']} ({actual!r})")
        else:
            if cell.HasFormula:
                raise DocumentError("a literal value became a formula")
            actual = cell.Value2
            expected = change["after"]
            if actual != expected and not (expected == "" and actual is None):
                raise DocumentError(f"cell read-back failed: {change['sheet']}!{change['cell']}")
    for op in ops:
        if op["kind"] in ("insert_rows", "delete_rows"):
            continue
        sheet = book.Worksheets.Item(op["sheet"])
        if op["kind"] == "delete_shape":
            if any(sheet.Shapes.Item(i).Name == op["name"] for i in range(1, sheet.Shapes.Count + 1)):
                raise DocumentError(f"shape deletion failed: {op['name']}")
        else:
            shape = _shape(sheet, op["name"])
            _verify_properties(shape, op.get("properties", {}))
            if op["kind"] == "add_connector":
                for which, key in (("Begin", "begin"), ("End", "end")):
                    connection = shape.ConnectorFormat
                    if not getattr(connection, which + "Connected") or getattr(connection, which + "ConnectedShape").Name != op[key]["name"]:
                        raise DocumentError("connector attachment read-back failed")


def _apply(app, source, intermediate, changes, ops, directory, rows):
    # Excel allows setting Calculation after a workbook exists. Keep a blank guard
    # open so the source and the verification copy both open in manual mode.
    guard = app.Workbooks.Add()
    try:
        app.Calculation = -4135
        app.CalculateBeforeSave = False
        _apply_book(app, source, intermediate, changes, ops, directory, rows)
    finally:
        guard.Close(SaveChanges=False)


def _apply_book(app, source, intermediate, changes, ops, directory, rows):
    book = _open(app, source)
    try:
        for op in rows:
            sheet = book.Worksheets.Item(op["sheet"])
            span = sheet.Rows(f"{op['at']}:{op['at'] + op['count'] - 1}")
            if op["kind"] == "insert_rows":
                span.Insert(Shift=-4121, CopyOrigin=0)
            else:
                span.Delete(Shift=-4162)
        # Copy formatting only after all coordinates have settled.
        for op in rows:
            if op["kind"] != "insert_rows" or "style_from" not in op:
                continue
            sheet = book.Worksheets.Item(op["sheet"])
            target = operations.resolve_target({"sheet": op["sheet"], "insertion": op["id"], "offset": 0, "column": "A"}, rows)
            _, start = operations.coordinate(target["cell"])
            template_row = operations.transform_row(op["sheet"], op["style_from"], rows)
            span = sheet.Rows(f"{start}:{start + op['count'] - 1}")
            sheet.Rows(template_row).Copy(Destination=span)
            span.ClearContents()
            span.RowHeight = sheet.Rows(template_row).RowHeight
        for change in changes:
            cell = book.Worksheets.Item(change["sheet"]).Range(change["cell"])
            if change.get("kind") == "formula":
                if change["after"] is None:
                    cell.ClearContents()
                else:
                    cell.Formula2 = change["after"]
            elif isinstance(change["after"], str):
                # A leading = or + is literal text for cell mappings.
                cell.Value2 = "'" + change["after"]
            else:
                cell.Value2 = change["after"]
        shape_ops = [o for o in ops if o["kind"] not in ("insert_rows", "delete_rows")]
        for op in sorted(shape_ops, key=lambda o: o["kind"] == "add_connector"):
            _edit_shape(book, op, directory)
        _verify(book, changes, ops)
        book.SaveCopyAs(str(intermediate))
    finally:
        book.Close(SaveChanges=False)
    reopened = _open(app, intermediate)
    try:
        _verify(reopened, changes, ops)
    finally:
        reopened.Close(SaveChanges=False)


def _patch(source: Path, destination: Path, changes: list[dict], ops: list[dict], directory: Path) -> dict:
    with ZipFile(source) as archive:
        if any(n.startswith("_xmlsignatures/") for n in archive.namelist()):
            raise DocumentError("signed Excel cannot be modified without invalidating its signature")
    rows = operations.validate_rows(ops, {s["name"] for s in excel.extract(source)})
    recalc = bool(rows) or any(c.get("kind") == "formula" for c in changes)
    recalc = recalc or bool(changes) and any(c["type"] == "formula" for s in excel.extract(source) for c in s["cells"])
    with tempfile.TemporaryDirectory(prefix="arp-native-", dir=destination.parent) as temp:
        intermediate = Path(temp) / ("saved" + source.suffix)
        with application() as app:
            try:
                _apply(app, source, intermediate, changes, ops, directory, rows)
            finally:
                # Release the caller's binding before the context uninitializes COM.
                app = None
        # Discard stale formula caches without recalculating externally linked formulas/UDFs.
        excel.patch(intermediate, destination, [], force_recalc=recalc)
        _verify_rows(source, destination, changes, rows)
    with ZipFile(source) as old, ZipFile(destination) as new:
        old_names, new_names = set(old.namelist()), set(new.namelist())
        changed = sorted(n for n in old_names | new_names if n not in old_names or n not in new_names or old.read(n) != new.read(n))
    return {"engine": "excel", "changed_parts": changed, "requires_excel_recalculation": recalc,
            "verified_after_reopen": True}


def _stop_owned_excel(owner_file: Path) -> None:
    """Only kill the new process recorded by our worker, with PID-reuse protection."""
    if not owner_file.is_file() or os.name != "nt":
        return
    import win32api
    import win32process
    import win32event
    owner = read(owner_file)
    try:
        handle = win32api.OpenProcess(0x100401, False, owner["pid"])
    except Exception:
        return  # Already exited.
    try:
        if win32event.WaitForSingleObject(handle, 2000) == 0:
            return
        if win32process.GetProcessTimes(handle)["CreationTime"].isoformat() == owner["created"]:
            win32api.TerminateProcess(handle, 1)
    finally:
        handle.Close()


def patch(source: Path, destination: Path, changes: list[dict], ops: list[dict], directory: Path) -> dict:
    """Isolate COM lifetime and cap hung Office dialogs; errors never publish a partial output."""
    if os.name != "nt":
        raise DocumentError("structural Excel writeback requires Windows + Microsoft Excel")
    with tempfile.TemporaryDirectory(prefix="arp-excel-job-", dir=destination.parent) as temp:
        job_path, result_path, owner_path = (Path(temp) / name for name in ("job.json", "result.json", "owner.json"))
        write(job_path, {"source": str(source.resolve()), "destination": str(destination.resolve()),
                        "directory": str(directory.resolve()), "changes": changes, "operations": ops})
        env = {**os.environ, "PYTHONPATH": os.pathsep.join(str(p) for p in sys.path if p), "PYTHONIOENCODING": "utf-8"}
        try:
            completed = subprocess.run([sys.executable, "-m", "arp4.documents.native_excel", str(job_path), str(result_path), str(owner_path)],
                capture_output=True, text=True, encoding="utf-8", env=env, timeout=120,
                creationflags=subprocess.CREATE_NO_WINDOW)
        except subprocess.TimeoutExpired as exc:
            _stop_owned_excel(owner_path)
            raise DocumentError("Excel writeback timed out after 120 seconds; no output adopted") from exc
        finally:
            # A crashed worker cannot leave our hidden Excel running indefinitely.
            _stop_owned_excel(owner_path)
        if not result_path.is_file():
            raise DocumentError(f"Excel worker failed: {completed.stderr.strip()}")
        result = read(result_path)
        if completed.returncode or "error" in result:
            raise DocumentError(result.get("error", "Excel worker failed"))
        return result


if __name__ == "__main__":
    _OWNER_FILE = Path(sys.argv[3])
    result_path = Path(sys.argv[2])
    try:
        job = read(Path(sys.argv[1]))
        result = _patch(Path(job["source"]), Path(job["destination"]), job["changes"], job["operations"], Path(job["directory"]))
        write(result_path, result)
    except Exception as exc:
        write(result_path, {"error": str(exc)})
        raise SystemExit(1)
