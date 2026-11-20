//! Bounded merged processor log and live preview.
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, PipeReader, Read, Seek, SeekFrom, Write},
    path::Path,
    process::Stdio,
};

use crate::data::registry::operation::message::output::OutputLine;

const MAX_LINES: usize = 6;
const MAX_LINE_BYTES: usize = 4096;
const MAX_READ_BYTES: u64 = 64 * 1024;
const LOG_LIMIT_NOTICE: &[u8] = b"\n[processor log limit reached]\n";

pub(super) struct OutputCapture {
    reader: PipeReader,
    log: File,
    remaining: u64,
    notice: &'static [u8],
}

impl OutputCapture {
    pub(super) fn open(log: &Path, limit: u64) -> io::Result<(Self, Stdio, Stdio)> {
        let (reader, writer) = io::pipe()?;
        set_read_nonblocking(&reader)?;
        let stdout = Stdio::from(writer.try_clone()?);
        let stderr = Stdio::from(writer);
        let notice = if limit >= LOG_LIMIT_NOTICE.len() as u64 {
            LOG_LIMIT_NOTICE
        } else {
            &[]
        };
        Ok((
            Self {
                reader,
                log: File::create(log)?,
                remaining: limit - notice.len() as u64,
                notice,
            },
            stdout,
            stderr,
        ))
    }

    /// Drain only bytes available now. A descendant retaining a pipe handle
    /// cannot delay publication or shutdown until it closes that handle.
    pub(super) fn drain(&mut self, mut budget: usize) -> io::Result<usize> {
        let mut buffer = [0; 16 * 1024];
        let mut read = 0;
        while budget > 0 {
            let requested = budget.min(buffer.len());
            let count = match read_available(&mut self.reader, &mut buffer[..requested]) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            };
            let keep = count.min(self.remaining.min(usize::MAX as u64) as usize);
            if keep > 0 {
                self.log.write_all(&buffer[..keep])?;
                self.remaining -= keep as u64;
            }
            if keep < count && !self.notice.is_empty() {
                self.log.write_all(std::mem::take(&mut self.notice))?;
            }
            budget -= count;
            read += count;
        }
        Ok(read)
    }
}

#[cfg(unix)]
fn set_read_nonblocking(reader: &PipeReader) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = reader.as_raw_fd();
    // The reader owns a valid descriptor throughout both calls.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_read_nonblocking(_: &PipeReader) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn read_available(reader: &mut PipeReader, buffer: &mut [u8]) -> io::Result<usize> {
    reader.read(buffer)
}

#[cfg(windows)]
fn read_available(reader: &mut PipeReader, buffer: &mut [u8]) -> io::Result<usize> {
    use std::{os::windows::io::AsRawHandle, ptr};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    let mut available = 0;
    // This capture owns the only reader. Peek and read operate serially on
    // the same handle and read never requests more bytes than are available.
    let result = unsafe {
        PeekNamedPipe(
            reader.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut available,
            ptr::null_mut(),
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::BrokenPipe {
            Ok(0)
        } else {
            Err(error)
        };
    }
    let count = (available as usize).min(buffer.len());
    if count == 0 {
        return Ok(0);
    }
    reader.read(&mut buffer[..count])
}

#[cfg(not(any(unix, windows)))]
fn read_available(_: &mut PipeReader, _: &mut [u8]) -> io::Result<usize> {
    Err(io::ErrorKind::Unsupported.into())
}

struct BufferedLine {
    id: u64,
    bytes: Vec<u8>,
    truncated: bool,
    closed: bool,
}

#[derive(Default)]
struct OutputBuffer {
    lines: VecDeque<BufferedLine>,
    next_id: u64,
    active: bool,
    pending_cr: bool,
}

impl OutputBuffer {
    fn active_line(&mut self) -> &mut BufferedLine {
        if !self.active {
            self.next_id += 1;
            self.lines.push_back(BufferedLine {
                id: self.next_id,
                bytes: Vec::new(),
                truncated: false,
                closed: false,
            });
            self.active = true;
            if self.lines.len() > MAX_LINES {
                self.lines.pop_front();
            }
        }
        self.lines.back_mut().expect("active line exists")
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.pending_cr {
                self.pending_cr = false;
                if byte != b'\n' {
                    let line = self.active_line();
                    line.bytes.clear();
                    line.truncated = false;
                }
            }

            match byte {
                b'\r' => {
                    self.active_line();
                    self.pending_cr = true;
                }
                b'\n' => {
                    self.active_line().closed = true;
                    self.active = false;
                }
                _ => {
                    let line = self.active_line();
                    if line.bytes.len() < MAX_LINE_BYTES {
                        line.bytes.push(byte);
                    } else {
                        line.truncated = true;
                    }
                }
            }
        }
    }

    fn snapshot(&self, finished: bool) -> Vec<OutputLine> {
        self.lines
            .iter()
            .map(|line| {
                let mut text = decode(&line.bytes, (line.closed || finished) && !line.truncated);
                if line.truncated {
                    text.push('…');
                }
                OutputLine { id: line.id, text }
            })
            .collect()
    }

    fn reset(&mut self) {
        self.lines.clear();
        self.active = false;
        self.pending_cr = false;
        // Preserve IDs across a skipped backlog or a truncated file.
    }
}

/// Incomplete UTF-8 waits for the next read; malformed bytes render lossily.
fn decode(mut bytes: &[u8], complete: bool) -> String {
    let mut text = String::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(&String::from_utf8_lossy(&bytes[..valid]));
                bytes = &bytes[valid..];
                match error.error_len() {
                    Some(len) => {
                        text.push('\u{fffd}');
                        bytes = &bytes[len..];
                    }
                    None => {
                        if complete {
                            text.push('\u{fffd}');
                        }
                        break;
                    }
                }
            }
        }
    }
    text
}

pub(super) struct OutputTail {
    file: File,
    offset: u64,
    buffer: OutputBuffer,
    last_snapshot: Vec<OutputLine>,
}

impl OutputTail {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            offset: 0,
            buffer: OutputBuffer::default(),
            last_snapshot: Vec::new(),
        })
    }

    pub fn poll(&mut self, finished: bool) -> io::Result<Option<Vec<OutputLine>>> {
        let end = self.file.metadata()?.len();
        if end < self.offset {
            self.offset = 0;
            self.buffer.reset();
        }

        let jumped = end - self.offset > MAX_READ_BYTES;
        if jumped {
            self.offset = end - MAX_READ_BYTES;
            self.buffer.reset();
        }

        self.file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut self.file)
            .take(end - self.offset)
            .read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;

        let bytes = if jumped {
            match bytes.iter().position(|&byte| byte == b'\n') {
                Some(index) if index + 1 < bytes.len() => &bytes[index + 1..],
                _ => {
                    // A single huge line still gets a bounded preview.
                    self.buffer.feed("…".as_bytes());
                    &bytes[..]
                }
            }
        } else {
            &bytes[..]
        };
        self.buffer.feed(bytes);

        let snapshot = self.buffer.snapshot(finished);
        if snapshot == self.last_snapshot {
            return Ok(None);
        }
        self.last_snapshot = snapshot.clone();
        Ok(Some(snapshot))
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
