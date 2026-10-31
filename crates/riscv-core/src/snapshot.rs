//! Bounded state serialization from pinned MIT-licensed rsemu.

/// Serializes the machine state.
#[derive(Default)]
pub struct StateWriter {
    buf: Vec<u8>,
}

impl StateWriter {
    pub fn new() -> StateWriter {
        StateWriter::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Writes a length-prefixed byte string.
    pub fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.buf.extend_from_slice(v);
    }

    /// Writes a named section with the contents written by f. A
    /// section is read back with [`StateReader::section`], which checks
    /// the name and that the reader consumes the whole section.
    pub fn section<F>(&mut self, name: &str, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut StateWriter) -> Result<(), String>,
    {
        self.bytes(name.as_bytes());
        let at = self.buf.len();
        self.u64(0);
        f(self)?;
        let len = (self.buf.len() - at - 8) as u64;
        self.buf[at..at + 8].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }
}

/// Deserializes the machine state written by a [`StateWriter`].
pub struct StateReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> StateReader<'a> {
    pub fn new(buf: &'a [u8]) -> StateReader<'a> {
        StateReader { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.buf.len() - self.pos < n {
            return Err("truncated snapshot state".to_string());
        }
        let b = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("invalid snapshot boolean".into()),
        }
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u64()?;
        self.take(usize::try_from(n).map_err(|_| "truncated snapshot state")?)
    }

    /// Reads the section written by [`StateWriter::section`].
    pub fn section<T, F>(&mut self, name: &str, f: F) -> Result<T, String>
    where
        F: FnOnce(&mut StateReader<'a>) -> Result<T, String>,
    {
        let found = self.bytes()?;
        if found != name.as_bytes() {
            return Err(format!(
                "snapshot: expected {} state, found {}",
                name,
                String::from_utf8_lossy(found)
            ));
        }
        let data = self.bytes()?;
        let mut r = StateReader::new(data);
        let v = f(&mut r).map_err(|e| format!("{}: {}", name, e))?;
        if r.pos != data.len() {
            return Err(format!(
                "snapshot: {} bytes of unknown {} state",
                data.len() - r.pos,
                name
            ));
        }
        Ok(v)
    }
}
