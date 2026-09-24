//! A worker thread for the save-database writes of gameplay (Tier C, not in the original).
//!
//! The original writes its SQLite saves where it produces them: the `time` blob inside the world
//! tick (0x0053254d), the character every 60 s in the mesher (0x00487520). An autocommit SQLite
//! write syncs the journal and the database file, a few milliseconds on an SSD and tens on a
//! hard disk, and the port's tick runs on the frame thread under the world lock, so those writes
//! showed as frame hitches. The frame thread hands them here instead. Jobs run one at a time in
//! submission order; [`SaveWriter::call`] runs after every job submitted before it, so a read
//! through it sees the earlier writes.

use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce() + Send>;

pub struct SaveWriter {
    tx: Mutex<Option<Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Default for SaveWriter {
    fn default() -> Self {
        SaveWriter::new()
    }
}

impl SaveWriter {
    pub fn new() -> SaveWriter {
        let (tx, rx) = channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("saves".into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("save thread");
        SaveWriter { tx: Mutex::new(Some(tx)), thread: Mutex::new(Some(thread)) }
    }

    /// Queues `job`. After [`SaveWriter::stop`] it runs on the caller's thread.
    pub fn submit(&self, job: impl FnOnce() + Send + 'static) {
        let job: Job = Box::new(job);
        let tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let job = match tx.as_ref() {
            Some(tx) => match tx.send(job) {
                Ok(()) => return,
                Err(e) => e.0,
            },
            None => job,
        };
        drop(tx);
        job();
    }

    /// Runs `f` after every job queued before it and returns its result.
    pub fn call<R: Send + 'static>(&self, f: impl FnOnce() -> R + Send + 'static) -> R {
        let (tx, rx) = channel();
        self.submit(move || {
            let _ = tx.send(f());
        });
        rx.recv().expect("save job")
    }

    /// Waits for every queued job.
    pub fn flush(&self) {
        self.call(|| ());
    }

    /// Runs the queued jobs and ends the thread.
    pub fn stop(&self) {
        drop(self.tx.lock().unwrap_or_else(|e| e.into_inner()).take());
        if let Some(t) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = t.join();
        }
    }
}

impl Drop for SaveWriter {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn jobs_run_off_the_caller_thread_in_order() {
        let w = SaveWriter::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        let me = std::thread::current().id();
        for i in 0..5 {
            let log = Arc::clone(&log);
            w.submit(move || {
                assert_ne!(std::thread::current().id(), me);
                std::thread::sleep(Duration::from_millis(2));
                log.lock().unwrap().push(i);
            });
        }
        // `call` sees every earlier job done.
        let seen = {
            let log = Arc::clone(&log);
            w.call(move || log.lock().unwrap().clone())
        };
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn submit_does_not_wait_for_the_job() {
        let w = SaveWriter::new();
        let (tx, rx) = channel::<()>();
        // The job waits for the caller: a synchronous submit would deadlock.
        w.submit(move || rx.recv().unwrap());
        tx.send(()).unwrap();
        w.flush();
    }

    #[test]
    fn stop_drains_the_queue_and_later_jobs_run_inline() {
        let w = SaveWriter::new();
        let n = Arc::new(Mutex::new(0));
        for _ in 0..3 {
            let n = Arc::clone(&n);
            w.submit(move || *n.lock().unwrap() += 1);
        }
        w.stop();
        assert_eq!(*n.lock().unwrap(), 3);
        let me = std::thread::current().id();
        let n2 = Arc::clone(&n);
        w.submit(move || {
            assert_eq!(std::thread::current().id(), me);
            *n2.lock().unwrap() += 1;
        });
        assert_eq!(*n.lock().unwrap(), 4);
        assert_eq!(w.call(|| 7), 7);
    }
}
