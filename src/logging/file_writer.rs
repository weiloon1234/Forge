use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

use crate::support::{Clock, Date};

#[derive(Clone)]
pub(crate) struct DateRotatingFileWriter {
    dir: PathBuf,
    clock: Clock,
    state: Arc<Mutex<DateRotatingState>>,
}

struct DateRotatingState {
    current_date: Date,
    file: File,
}

impl DateRotatingFileWriter {
    pub(crate) fn open(dir: &str, clock: &Clock) -> io::Result<Self> {
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir)?;

        let today = clock.today();
        let file = open_date_file(&dir, &today)?;

        Ok(Self {
            dir,
            clock: clock.clone(),
            state: Arc::new(Mutex::new(DateRotatingState {
                current_date: today,
                file,
            })),
        })
    }
}

fn open_date_file(dir: &std::path::Path, date: &Date) -> io::Result<File> {
    let path = dir.join(format!("{date}.log"));
    OpenOptions::new().create(true).append(true).open(path)
}

pub(crate) struct FileWriterGuard<'a> {
    guard: MutexGuard<'a, DateRotatingState>,
}

use std::sync::MutexGuard;

impl<'a> MakeWriter<'a> for DateRotatingFileWriter {
    type Writer = FileWriterGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        let mut state = self.state.lock().expect("log file lock poisoned");

        let today = self.clock.today();
        if today != state.current_date {
            if let Ok(file) = open_date_file(&self.dir, &today) {
                state.file = file;
                state.current_date = today;
            }
        }

        FileWriterGuard { guard: state }
    }
}

impl<'a> io::Write for FileWriterGuard<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.guard.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.guard.file.flush()
    }
}
