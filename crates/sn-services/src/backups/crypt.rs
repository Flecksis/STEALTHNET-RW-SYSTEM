use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

pub const MAGIC: &[u8; 8] = b"SNBACKUP";
const VERSION: u8 = 1;
const KDF_ARGON2ID: u8 = 1;
const SALT_LEN: usize = 16;
const PREFIX_LEN: usize = 7;
const CHECK_LEN: usize = 8;
const TAG_LEN: usize = 16;
pub const CHUNK: usize = 1024 * 1024;
const PUBLIC_MAX: usize = 16 * 1024;
const MEMORY_MAX_KIB: u32 = 1024 * 1024;
const PASSES_MAX: u32 = 16;
const LANES_MAX: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

impl KdfParams {
    pub const STRONG: KdfParams = KdfParams {
        memory_kib: 64 * 1024,
        passes: 3,
        lanes: 1,
    };

    pub const QUICK: KdfParams = KdfParams {
        memory_kib: 1024,
        passes: 1,
        lanes: 1,
    };
}

#[derive(Debug, thiserror::Error)]
pub enum CryptError {
    #[error("это не зашифрованная копия STEALTHNET")]
    NotSealed,
    #[error("копия зашифрована более новой версией панели")]
    Version,
    #[error("пароль не подошёл")]
    WrongPassword,
    #[error("файл копии повреждён: {0}")]
    Damaged(&'static str),
    #[error("файл копии обрезан: он скачан или передан не до конца")]
    Truncated,
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl From<CryptError> for io::Error {
    fn from(err: CryptError) -> Self {
        match err {
            CryptError::Io(err) => err,
            CryptError::Truncated => io::Error::new(io::ErrorKind::UnexpectedEof, err.to_string()),
            other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
        }
    }
}

fn derive(password: &str, salt: &[u8], params: KdfParams) -> io::Result<[u8; 32]> {
    let params = Params::new(params.memory_kib, params.passes, params.lanes, Some(32))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    Ok(key)
}

fn password_check(key: &[u8; 32]) -> [u8; CHECK_LEN] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("ключ любой длины");
    mac.update(b"stealthnet-backup-password-check");
    let full = mac.finalize().into_bytes();
    let mut out = [0u8; CHECK_LEN];
    out.copy_from_slice(&full[..CHECK_LEN]);
    out
}

fn nonce(prefix: &[u8; PREFIX_LEN], counter: u32, last: bool) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..PREFIX_LEN].copy_from_slice(prefix);
    nonce[PREFIX_LEN..PREFIX_LEN + 4].copy_from_slice(&counter.to_be_bytes());
    nonce[11] = u8::from(last);
    nonce
}

pub struct Header {
    pub params: KdfParams,
    pub salt: [u8; SALT_LEN],
    pub prefix: [u8; PREFIX_LEN],
    pub check: [u8; CHECK_LEN],
    pub public: Vec<u8>,
}

impl Header {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.public.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(KDF_ARGON2ID);
        out.extend_from_slice(&self.params.memory_kib.to_be_bytes());
        out.extend_from_slice(&self.params.passes.to_be_bytes());
        out.extend_from_slice(&self.params.lanes.to_be_bytes());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.prefix);
        out.extend_from_slice(&self.check);
        out.extend_from_slice(&(self.public.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.public);
        out
    }

    pub fn read(source: &mut impl Read) -> Result<(Header, Vec<u8>), CryptError> {
        let mut magic = [0u8; 8];
        read_exact(source, &mut magic)?;
        if &magic != MAGIC {
            return Err(CryptError::NotSealed);
        }
        let mut fixed = [0u8; 2 + 12 + SALT_LEN + PREFIX_LEN + CHECK_LEN + 4];
        read_exact(source, &mut fixed)?;
        if fixed[0] != VERSION || fixed[1] != KDF_ARGON2ID {
            return Err(CryptError::Version);
        }
        let word = |at: usize| {
            u32::from_be_bytes([fixed[at], fixed[at + 1], fixed[at + 2], fixed[at + 3]])
        };
        let params = KdfParams {
            memory_kib: word(2),
            passes: word(6),
            lanes: word(10),
        };
        if params.memory_kib == 0
            || params.memory_kib > MEMORY_MAX_KIB
            || params.passes == 0
            || params.passes > PASSES_MAX
            || params.lanes == 0
            || params.lanes > LANES_MAX
        {
            return Err(CryptError::Damaged("неверные параметры пароля"));
        }
        let mut at = 14;
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&fixed[at..at + SALT_LEN]);
        at += SALT_LEN;
        let mut prefix = [0u8; PREFIX_LEN];
        prefix.copy_from_slice(&fixed[at..at + PREFIX_LEN]);
        at += PREFIX_LEN;
        let mut check = [0u8; CHECK_LEN];
        check.copy_from_slice(&fixed[at..at + CHECK_LEN]);
        at += CHECK_LEN;
        let len = word(at) as usize;
        if len > PUBLIC_MAX {
            return Err(CryptError::Damaged("слишком длинный заголовок"));
        }
        let mut public = vec![0u8; len];
        read_exact(source, &mut public)?;
        let header = Header {
            params,
            salt,
            prefix,
            check,
            public,
        };
        let raw = header.encode();
        Ok((header, raw))
    }
}

fn read_exact(source: &mut impl Read, buf: &mut [u8]) -> Result<(), CryptError> {
    source.read_exact(buf).map_err(|err| {
        if err.kind() == io::ErrorKind::UnexpectedEof {
            CryptError::Truncated
        } else {
            CryptError::Io(err)
        }
    })
}

pub struct Sealer<W: Write> {
    inner: W,
    cipher: Aes256Gcm,
    prefix: [u8; PREFIX_LEN],
    aad: [u8; 32],
    counter: u32,
    pending: Vec<u8>,
}

impl<W: Write> Sealer<W> {
    pub fn new(mut inner: W, password: &str, params: KdfParams, public: &[u8]) -> io::Result<Self> {
        let mut salt = [0u8; SALT_LEN];
        let mut prefix = [0u8; PREFIX_LEN];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        rand::rngs::OsRng.fill_bytes(&mut prefix);
        let key = derive(password, &salt, params)?;
        let header = Header {
            params,
            salt,
            prefix,
            check: password_check(&key),
            public: public.to_vec(),
        };
        let raw = header.encode();
        inner.write_all(&raw)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        Ok(Sealer {
            inner,
            cipher,
            prefix,
            aad: Sha256::digest(&raw).into(),
            counter: 0,
            pending: Vec::with_capacity(CHUNK),
        })
    }

    fn seal(&mut self, last: bool) -> io::Result<()> {
        let nonce = nonce(&self.prefix, self.counter, last);
        let sealed = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &self.pending,
                    aad: &self.aad,
                },
            )
            .map_err(|_| io::Error::other("копия не зашифровалась"))?;
        self.inner.write_all(&(sealed.len() as u32).to_be_bytes())?;
        self.inner.write_all(&sealed)?;
        self.pending.clear();
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| io::Error::other("копия слишком большая"))?;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.seal(true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for Sealer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        while written < buf.len() {
            if self.pending.len() == CHUNK {
                self.seal(false)?;
            }
            let room = CHUNK - self.pending.len();
            let take = room.min(buf.len() - written);
            self.pending
                .extend_from_slice(&buf[written..written + take]);
            written += take;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub struct Opener<R: Read> {
    inner: R,
    cipher: Aes256Gcm,
    prefix: [u8; PREFIX_LEN],
    aad: [u8; 32],
    counter: u32,
    plain: Vec<u8>,
    at: usize,
    done: bool,
}

impl<R: Read> Opener<R> {
    pub fn new(mut inner: R, password: &str) -> Result<(Self, Vec<u8>), CryptError> {
        let (header, raw) = Header::read(&mut inner)?;
        let key = derive(password, &header.salt, header.params)?;
        if password_check(&key) != header.check {
            return Err(CryptError::WrongPassword);
        }
        let cipher =
            Aes256Gcm::new_from_slice(&key).map_err(|_| CryptError::Damaged("неверный ключ"))?;
        let public = header.public.clone();
        Ok((
            Opener {
                inner,
                cipher,
                prefix: header.prefix,
                aad: Sha256::digest(&raw).into(),
                counter: 0,
                plain: Vec::new(),
                at: 0,
                done: false,
            },
            public,
        ))
    }

    fn next_chunk(&mut self) -> Result<(), CryptError> {
        let mut len = [0u8; 4];
        read_exact(&mut self.inner, &mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if !(TAG_LEN..=CHUNK + TAG_LEN).contains(&len) {
            return Err(CryptError::Damaged("неверная длина блока"));
        }
        let mut sealed = vec![0u8; len];
        read_exact(&mut self.inner, &mut sealed)?;
        let attempt = |last: bool| {
            self.cipher.decrypt(
                Nonce::from_slice(&nonce(&self.prefix, self.counter, last)),
                Payload {
                    msg: &sealed,
                    aad: &self.aad,
                },
            )
        };
        let (plain, last) = match attempt(false) {
            Ok(plain) => (plain, false),
            Err(_) => match attempt(true) {
                Ok(plain) => (plain, true),
                Err(_) => return Err(CryptError::Damaged("блок данных не сходится")),
            },
        };
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or(CryptError::Damaged("слишком много блоков"))?;
        if last {
            let mut probe = [0u8; 1];
            match self.inner.read(&mut probe) {
                Ok(0) => {}
                Ok(_) => return Err(CryptError::Damaged("после конца копии есть лишние данные")),
                Err(err) => return Err(CryptError::Io(err)),
            }
            self.done = true;
        }
        self.plain = plain;
        self.at = 0;
        Ok(())
    }
}

impl<R: Read> Read for Opener<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.at == self.plain.len() {
            if self.done {
                return Ok(0);
            }
            self.next_chunk()?;
        }
        let take = buf.len().min(self.plain.len() - self.at);
        buf[..take].copy_from_slice(&self.plain[self.at..self.at + take]);
        self.at += take;
        Ok(take)
    }
}

pub fn read_public(source: &mut impl Read) -> Result<Vec<u8>, CryptError> {
    Ok(Header::read(source)?.0.public)
}

pub fn key_check(key: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("ключ любой длины");
    mac.update(b"stealthnet-backup-key-check");
    hex::encode(&mac.finalize().into_bytes()[..CHECK_LEN])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(data: &[u8], password: &str) -> Vec<u8> {
        let mut sealer = Sealer::new(Vec::new(), password, KdfParams::QUICK, b"{\"x\":1}").unwrap();
        for piece in data.chunks(777) {
            sealer.write_all(piece).unwrap();
        }
        sealer.finish().unwrap()
    }

    fn open(sealed: &[u8], password: &str) -> Result<Vec<u8>, io::Error> {
        let (mut opener, public) = Opener::new(sealed, password).map_err(io::Error::from)?;
        assert_eq!(public, b"{\"x\":1}");
        let mut out = Vec::new();
        opener.read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn шифрование_туда_и_обратно_на_границах_блоков() {
        for size in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 2 * CHUNK + 5] {
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let sealed = seal(&data, "пароль копии");
            assert_eq!(
                open(&sealed, "пароль копии").unwrap(),
                data,
                "размер {size}"
            );
        }
    }

    #[test]
    fn неверный_пароль_распознаётся_сразу() {
        let sealed = seal(b"data", "верный");
        assert!(matches!(
            Opener::new(sealed.as_slice(), "неверный"),
            Err(CryptError::WrongPassword)
        ));
    }

    #[test]
    fn обрезанный_и_испорченный_файл_не_читается() {
        let data = vec![7u8; CHUNK + 10];
        let sealed = seal(&data, "p");
        let cut = &sealed[..sealed.len() - 20];
        let err = open(cut, "p").unwrap_err();
        assert!(err.to_string().contains("обрезан"), "{err}");
        let without_last = &sealed[..sealed.len() - (4 + 10 + TAG_LEN)];
        let err = open(without_last, "p").unwrap_err();
        assert!(err.to_string().contains("обрезан"), "{err}");
        let mut broken = sealed.clone();
        let middle = broken.len() / 2;
        broken[middle] ^= 0x40;
        let err = open(&broken, "p").unwrap_err();
        assert!(err.to_string().contains("повреждён"), "{err}");
        let mut tail = sealed.clone();
        tail.push(0);
        let err = open(&tail, "p").unwrap_err();
        assert!(err.to_string().contains("лишние"), "{err}");
        let mut header = sealed;
        header[20] ^= 1;
        assert!(Opener::new(header.as_slice(), "p").is_err());
    }

    #[test]
    fn чужой_файл_не_принимается_за_копию() {
        assert!(matches!(
            Opener::new(&b"\x1f\x8b\x08\x00gzip"[..], "p"),
            Err(CryptError::NotSealed)
        ));
        assert!(matches!(
            Opener::new(&b"SNBA"[..], "p"),
            Err(CryptError::Truncated)
        ));
    }

    #[test]
    fn отпечаток_ключа_не_раскрывает_ключ_и_различает_ключи() {
        let a = key_check(&[1u8; 32]);
        let b = key_check(&[2u8; 32]);
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
        assert_eq!(a, key_check(&[1u8; 32]));
    }
}
