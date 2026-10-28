use yinz_cli::session_picker::PickerRow;
pub struct Discovery {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
    replies: std::sync::mpsc::Receiver<(usize, Vec<PickerRow>)>,
    hosts: Vec<Vec<PickerRow>>,
}

fn host_status(host: &str, status: &str) -> Vec<PickerRow> {
    vec![PickerRow {
        label: host.to_owned(),
        destination: None,
        status: Some(status.to_owned()),
        age: std::time::Duration::ZERO,
        listing: yinz_cli::sessions::SessionListing::Denied,
        current: false,
    }]
}

impl Discovery {
    pub fn start(hosts: Vec<String>) -> Self {
        Self::start_with(hosts, |host, cancelled| {
            crate::remote_sessions::discover(host, cancelled, &Default::default())
        })
    }

    pub fn start_with(
        hosts: Vec<String>,
        query: impl Fn(&str, &std::sync::atomic::AtomicBool) -> anyhow::Result<Vec<PickerRow>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        let query = Arc::new(query);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (send, replies) = std::sync::mpsc::channel();
        let rows = hosts
            .iter()
            .map(|host| host_status(host, "loading…"))
            .collect();
        let count = hosts.len().min(4);
        let pending = Arc::new(Mutex::new(hosts.into_iter().enumerate()));
        let workers = (0..count)
            .map(|_| {
                let query = query.clone();
                let pending = pending.clone();
                let cancelled = cancelled.clone();
                let send = send.clone();
                std::thread::spawn(move || loop {
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let next = pending.lock().unwrap_or_else(|e| e.into_inner()).next();
                    let Some((index, host)) = next else {
                        break;
                    };
                    let rows = match query(&host, &cancelled) {
                        Ok(rows) if rows.is_empty() => host_status(&host, "no live sessions"),
                        Ok(rows) => rows,
                        Err(error) => {
                            log::warn!("remote session discovery on {host}: {error:#}");
                            host_status(&host, "discovery failed; check SSH and Yinz versions")
                        },
                    };
                    if send.send((index, rows)).is_err() {
                        break;
                    }
                })
            })
            .collect();
        Self {
            cancelled,
            workers,
            replies,
            hosts: rows,
        }
    }

    pub fn update(&mut self) -> bool {
        let mut changed = false;
        for (index, rows) in self.replies.try_iter() {
            self.hosts[index] = rows;
            changed = true;
        }
        changed
    }

    pub fn rows(&self, local: &[PickerRow]) -> Vec<PickerRow> {
        local
            .iter()
            .chain(self.hosts.iter().flatten())
            .cloned()
            .collect()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                log::error!("remote discovery worker panicked");
            }
        }
    }
}
