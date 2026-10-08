//! Office's encryption of Open XML packages ([MS-OFFCRYPTO]).
//!
//! An encrypted package is stored in the `EncryptedPackage` stream of an OLE
//! compound file. With a password, `EncryptionInfo` describes how the key
//! follows from it: Agile encryption (Office 2010 and later) or Standard
//! encryption (Office 2007). Rights management (IRM and sensitivity labels)
//! uses the same container without `EncryptionInfo`; its key is issued by the
//! rights management service to a signed-in user, so only Office removes it.

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use roxmltree::{Document, Node};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};
use std::io::{Cursor, Read};

const OLE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const PASSWORD_ENCRYPTOR: &str = "http://schemas.microsoft.com/office/2006/keyEncryptor/password";
/// Agile encryption encrypts the package in segments of this size.
const SEGMENT: usize = 4096;
/// The largest package decrypted, as for unencrypted originals.
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;

/// How an Office file is encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// Encrypted with a password ("Encrypt with Password").
    Password,
    /// Encrypted by rights management: IRM or a sensitivity label.
    RightsManagement,
}

impl Protection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::RightsManagement => "rights_management",
        }
    }
}

/// How `raw` is encrypted, or `None` when it is not an encrypted Office package.
pub fn protection(raw: &[u8]) -> Option<Protection> {
    if !raw.starts_with(&OLE) {
        return None;
    }
    let file = cfb::CompoundFile::open(Cursor::new(raw)).ok()?;
    if !file.is_stream("/EncryptedPackage") {
        return None;
    }
    Some(if file.is_stream("/EncryptionInfo") {
        Protection::Password
    } else {
        Protection::RightsManagement
    })
}

/// The package of a password-encrypted file, decrypted with the first of
/// `passwords` that opens it.
pub fn decrypt(raw: &[u8], passwords: &[String]) -> Result<Vec<u8>> {
    let mut file = cfb::CompoundFile::open(Cursor::new(raw)).context("not an OLE compound file")?;
    let info = stream(&mut file, "/EncryptionInfo")?;
    let package = stream(&mut file, "/EncryptedPackage")?;
    ensure!(
        package.len() >= 8,
        "the encrypted package is shorter than its size field"
    );
    let size = u64::from_le_bytes(package[..8].try_into()?);
    ensure!(
        size <= MAX_PACKAGE && size <= (package.len() - 8) as u64,
        "the encrypted package declares {size} bytes, more than it holds or the size budget allows"
    );
    let scheme = Scheme::parse(&info)?;
    ensure!(
        !passwords.is_empty(),
        "the file is encrypted with a password; pass it on standard input with --password-stdin"
    );
    for password in passwords {
        if let Some(key) = scheme.key(password)? {
            let mut plain = scheme.decrypt_package(&key, &package)?;
            ensure!(
                plain.len() as u64 >= size,
                "the encrypted package is shorter than its declared size"
            );
            plain.truncate(size as usize);
            return Ok(plain);
        }
    }
    bail!(
        "none of the {} password(s) given opens the file",
        passwords.len()
    )
}

fn stream(file: &mut cfb::CompoundFile<Cursor<&[u8]>>, path: &str) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    file.open_stream(path)
        .with_context(|| format!("the encrypted file has no {path} stream"))?
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[derive(Clone, Copy)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn named(name: &str) -> Result<Self> {
        Ok(match name {
            "SHA1" | "SHA-1" => Self::Sha1,
            "SHA256" => Self::Sha256,
            "SHA384" => Self::Sha384,
            "SHA512" => Self::Sha512,
            _ => bail!("unsupported encryption hash algorithm {name}"),
        })
    }

    fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut hasher = D::new();
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().to_vec()
        }
        match self {
            Self::Sha1 => run::<Sha1>(parts),
            Self::Sha256 => run::<Sha256>(parts),
            Self::Sha384 => run::<Sha384>(parts),
            Self::Sha512 => run::<Sha512>(parts),
        }
    }

    fn block_size(self) -> usize {
        match self {
            Self::Sha1 | Self::Sha256 => 64,
            Self::Sha384 | Self::Sha512 => 128,
        }
    }

    /// HMAC (RFC 2104) of `message` under `key`.
    fn hmac(self, key: &[u8], message: &[u8]) -> Vec<u8> {
        let mut block = if key.len() > self.block_size() {
            self.digest(&[key])
        } else {
            key.to_vec()
        };
        block.resize(self.block_size(), 0);
        let inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
        let outer: Vec<u8> = block.iter().map(|b| b ^ 0x5C).collect();
        let inner = self.digest(&[&inner, message]);
        self.digest(&[&outer, &inner])
    }

    /// The password hash both schemes start from: the salted hash of the
    /// UTF-16LE password, hashed again `spins` times with the iteration number.
    fn spun(self, salt: &[u8], password: &str, spins: u32) -> Vec<u8> {
        let password: Vec<u8> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut hash = self.digest(&[salt, &password]);
        for i in 0..spins {
            hash = self.digest(&[&i.to_le_bytes(), &hash]);
        }
        hash
    }
}

/// `bytes` cut or padded with 0x36 to `length`, as keys and IVs are sized.
fn sized(mut bytes: Vec<u8>, length: usize) -> Vec<u8> {
    bytes.resize(length, 0x36);
    bytes
}

enum Aes {
    Aes128(aes::Aes128),
    Aes192(aes::Aes192),
    Aes256(aes::Aes256),
}

impl Aes {
    fn new(key: &[u8]) -> Result<Self> {
        use aes::cipher::KeyInit;
        Ok(match key.len() {
            16 => Self::Aes128(aes::Aes128::new_from_slice(key)?),
            24 => Self::Aes192(aes::Aes192::new_from_slice(key)?),
            32 => Self::Aes256(aes::Aes256::new_from_slice(key)?),
            n => bail!("unsupported AES key length of {} bits", n * 8),
        })
    }

    fn decrypt_block(&self, block: &mut [u8]) {
        use aes::cipher::{Array, BlockCipherDecrypt};
        let block = <&mut Array<u8, _>>::try_from(block).expect("AES block");
        match self {
            Self::Aes128(cipher) => cipher.decrypt_block(block),
            Self::Aes192(cipher) => cipher.decrypt_block(block),
            Self::Aes256(cipher) => cipher.decrypt_block(block),
        }
    }

    fn ecb(&self, data: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            data.len().is_multiple_of(16),
            "encrypted data is not a whole number of AES blocks"
        );
        let mut plain = data.to_vec();
        for block in plain.as_chunks_mut::<16>().0 {
            self.decrypt_block(block);
        }
        Ok(plain)
    }

    fn cbc(&self, iv: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            data.len().is_multiple_of(16),
            "encrypted data is not a whole number of AES blocks"
        );
        let mut plain = data.to_vec();
        let mut previous = iv;
        let blocks = data.as_chunks::<16>().0;
        for (block, cipher) in plain.as_chunks_mut::<16>().0.iter_mut().zip(blocks) {
            self.decrypt_block(block);
            for (byte, chained) in block.iter_mut().zip(previous) {
                *byte ^= chained;
            }
            previous = cipher;
        }
        Ok(plain)
    }
}

/// The parameters of one AES key in Agile encryption: `keyData` for the
/// package and `encryptedKey` for the password.
struct KeyParameters {
    salt: Vec<u8>,
    block_size: usize,
    key_bytes: usize,
    hash: Hash,
}

impl KeyParameters {
    fn parse(node: Node<'_, '_>) -> Result<Self> {
        let attribute = |name: &str| {
            node.attribute(name)
                .with_context(|| format!("{} has no {name}", node.tag_name().name()))
        };
        let cipher = attribute("cipherAlgorithm")?;
        ensure!(
            cipher == "AES",
            "unsupported encryption cipher {cipher}; only AES is supported"
        );
        let chaining = attribute("cipherChaining")?;
        ensure!(
            chaining == "ChainingModeCBC",
            "unsupported cipher chaining {chaining}"
        );
        let block_size: usize = attribute("blockSize")?.parse()?;
        ensure!(
            block_size == 16,
            "AES uses 16-byte blocks, not {block_size}"
        );
        let bits: usize = attribute("keyBits")?.parse()?;
        Ok(Self {
            salt: base64(attribute("saltValue")?)?,
            block_size,
            key_bytes: bits / 8,
            hash: Hash::named(attribute("hashAlgorithm")?)?,
        })
    }

    fn iv(&self, block_key: &[u8]) -> Vec<u8> {
        sized(self.hash.digest(&[&self.salt, block_key]), self.block_size)
    }
}

fn base64(value: &str) -> Result<Vec<u8>> {
    Ok(STANDARD.decode(value.trim())?)
}

enum Scheme {
    Agile {
        data: KeyParameters,
        password: KeyParameters,
        spins: u32,
        verifier_input: Vec<u8>,
        verifier_hash: Vec<u8>,
        key: Vec<u8>,
        integrity: Option<(Vec<u8>, Vec<u8>)>,
    },
    Standard {
        salt: Vec<u8>,
        key_bytes: usize,
        verifier: Vec<u8>,
        verifier_hash: Vec<u8>,
    },
}

impl Scheme {
    fn parse(info: &[u8]) -> Result<Self> {
        ensure!(info.len() >= 8, "EncryptionInfo is truncated");
        let major = u16::from_le_bytes([info[0], info[1]]);
        let minor = u16::from_le_bytes([info[2], info[3]]);
        match (major, minor) {
            (4, 4) => Self::agile(&info[8..]),
            (2..=4, 2) => Self::standard(info),
            (3 | 4, 3) => bail!("extensible encryption is not supported"),
            _ => bail!("unsupported encryption version {major}.{minor}"),
        }
    }

    fn agile(xml: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(xml).context("EncryptionInfo is not UTF-8")?;
        let doc = Document::parse(text.trim_start_matches('\u{feff}'))?;
        let element = |name: &str| {
            doc.descendants()
                .find(|node| node.is_element() && node.tag_name().name() == name)
        };
        let data = element("keyData").context("EncryptionInfo has no keyData")?;
        let encryptor = doc
            .descendants()
            .find(|node| {
                node.tag_name().name() == "keyEncryptor"
                    && node.attribute("uri") == Some(PASSWORD_ENCRYPTOR)
            })
            .and_then(|node| {
                node.children()
                    .find(|child| child.tag_name().name() == "encryptedKey")
            })
            .context("the file is not encrypted with a password")?;
        let value = |name: &str| {
            base64(
                encryptor
                    .attribute(name)
                    .with_context(|| format!("encryptedKey has no {name}"))?,
            )
        };
        let integrity = match element("dataIntegrity") {
            Some(node) => {
                let value = |name: &str| {
                    base64(
                        node.attribute(name)
                            .with_context(|| format!("dataIntegrity has no {name}"))?,
                    )
                };
                Some((value("encryptedHmacKey")?, value("encryptedHmacValue")?))
            }
            None => None,
        };
        Ok(Self::Agile {
            data: KeyParameters::parse(data)?,
            password: KeyParameters::parse(encryptor)?,
            spins: encryptor
                .attribute("spinCount")
                .context("encryptedKey has no spinCount")?
                .parse()?,
            verifier_input: value("encryptedVerifierHashInput")?,
            verifier_hash: value("encryptedVerifierHashValue")?,
            key: value("encryptedKeyValue")?,
            integrity,
        })
    }

    fn standard(info: &[u8]) -> Result<Self> {
        let u32_at = |at: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(
                info.get(at..at + 4)
                    .context("EncryptionInfo is truncated")?
                    .try_into()?,
            ))
        };
        let flags = u32_at(4)?;
        ensure!(
            flags & 0x24 == 0x24 && flags & 0x10 == 0,
            "unsupported Standard encryption: only AES is supported"
        );
        let header_size = u32_at(8)? as usize;
        let header = 12;
        let algorithm = u32_at(header + 8)?;
        ensure!(
            matches!(algorithm, 0x660E..=0x6610),
            "unsupported Standard encryption algorithm {algorithm:#x}"
        );
        let hash = u32_at(header + 12)?;
        ensure!(
            hash == 0 || hash == 0x8004,
            "unsupported Standard encryption hash {hash:#x}"
        );
        let key_bits = u32_at(header + 16)? as usize;
        let verifier = header + header_size;
        let salt_size = u32_at(verifier)? as usize;
        ensure!(salt_size == 16, "unsupported salt size {salt_size}");
        let bytes = |from: usize, length: usize| -> Result<Vec<u8>> {
            Ok(info
                .get(from..from + length)
                .context("EncryptionInfo is truncated")?
                .to_vec())
        };
        Ok(Self::Standard {
            salt: bytes(verifier + 4, 16)?,
            key_bytes: key_bits / 8,
            verifier: bytes(verifier + 20, 16)?,
            verifier_hash: bytes(verifier + 40, 32)?,
        })
    }

    /// The key that decrypts the package, or `None` when `password` is wrong.
    fn key(&self, password: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Agile {
                password: parameters,
                spins,
                verifier_input,
                verifier_hash,
                key,
                data,
                ..
            } => {
                let hash = parameters.hash;
                let spun = hash.spun(&parameters.salt, password, *spins);
                let decrypt = |block_key: &[u8], encrypted: &[u8]| {
                    let key = sized(hash.digest(&[&spun, block_key]), parameters.key_bytes);
                    Aes::new(&key)?.cbc(&parameters.salt, encrypted)
                };
                let input = decrypt(
                    &[0xFE, 0xA7, 0xD2, 0x76, 0x3B, 0x4B, 0x9E, 0x79],
                    verifier_input,
                )?;
                let expected = decrypt(
                    &[0xD7, 0xAA, 0x0F, 0x6D, 0x30, 0x61, 0x34, 0x4E],
                    verifier_hash,
                )?;
                let input = input
                    .get(..parameters.salt.len())
                    .context("verifier is truncated")?;
                let actual = hash.digest(&[input]);
                if expected.get(..actual.len()) != Some(actual.as_slice()) {
                    return Ok(None);
                }
                let mut secret = decrypt(&[0x14, 0x6E, 0x0B, 0xE7, 0xAB, 0xAC, 0xD0, 0xD6], key)?;
                ensure!(
                    secret.len() >= data.key_bytes,
                    "the encrypted key is truncated"
                );
                secret.truncate(data.key_bytes);
                Ok(Some(secret))
            }
            Self::Standard {
                salt,
                key_bytes,
                verifier,
                verifier_hash,
            } => {
                let hash = Hash::Sha1;
                let spun = hash.spun(salt, password, 50_000);
                let last = hash.digest(&[&spun, &0u32.to_le_bytes()]);
                let derived = |fill: u8| {
                    let mut buffer = [fill; 64];
                    for (byte, h) in buffer.iter_mut().zip(&last) {
                        *byte ^= h;
                    }
                    hash.digest(&[&buffer])
                };
                let mut key = derived(0x36);
                key.extend(derived(0x5C));
                key.truncate(*key_bytes);
                let cipher = Aes::new(&key)?;
                let verifier = cipher.ecb(verifier)?;
                let expected = cipher.ecb(verifier_hash)?;
                Ok((expected[..20] == hash.digest(&[&verifier])[..]).then_some(key))
            }
        }
    }

    /// The decrypted package, with the size field removed and the padding of
    /// the last block left on.
    fn decrypt_package(&self, key: &[u8], package: &[u8]) -> Result<Vec<u8>> {
        let cipher = Aes::new(key)?;
        let encrypted = &package[8..];
        match self {
            Self::Agile {
                data, integrity, ..
            } => {
                if let Some((hmac_key, hmac_value)) = integrity {
                    let decrypt = |block_key: &[u8], value: &[u8]| -> Result<Vec<u8>> {
                        let mut plain = cipher.cbc(&data.iv(block_key), value)?;
                        plain.truncate(data.hash.digest(&[]).len());
                        Ok(plain)
                    };
                    let hmac_key =
                        decrypt(&[0x5F, 0xB2, 0xAD, 0x01, 0x0C, 0xB9, 0xE1, 0xF6], hmac_key)?;
                    let expected = decrypt(
                        &[0xA0, 0x67, 0x7F, 0x02, 0xB2, 0x2C, 0x84, 0x33],
                        hmac_value,
                    )?;
                    ensure!(
                        data.hash.hmac(&hmac_key, package) == expected,
                        "the encrypted package fails its integrity check; the file is damaged"
                    );
                }
                let mut plain = Vec::with_capacity(encrypted.len());
                for (index, segment) in encrypted.chunks(SEGMENT).enumerate() {
                    let iv = data.iv(&(index as u32).to_le_bytes());
                    plain.extend(cipher.cbc(&iv, segment)?);
                }
                Ok(plain)
            }
            Self::Standard { .. } => {
                let whole = encrypted.len() - encrypted.len() % 16;
                cipher.ecb(&encrypted[..whole])
            }
        }
    }
}
