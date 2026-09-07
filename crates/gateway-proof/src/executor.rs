use std::{future::Future, sync::mpsc};

use sqlx::PgPool;

use crate::Error;

type Job = std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;

pub(crate) struct Executor {
    pool: PgPool,
    jobs: mpsc::Sender<Job>,
}

impl Executor {
    pub(crate) fn connect(url: &str) -> Result<Self, Error> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("proof-gateway-db")
            .enable_all()
            .build()
            .map_err(|_| Error::Unavailable)?;
        let (jobs, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("proof-gateway-db".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    rt.spawn(job);
                }
            })
            .map_err(|_| Error::Unavailable)?;
        let url = url.to_owned();
        let pool = submit(&jobs, async move {
            db::connect(&url).await.map_err(|_| Error::Unavailable)
        })?;
        Ok(Self { pool, jobs })
    }

    pub(crate) fn run<F, T>(&self, f: impl FnOnce(PgPool) -> F) -> Result<T, Error>
    where
        F: Future<Output = Result<T, Error>> + Send + 'static,
        T: Send + 'static,
    {
        submit(&self.jobs, f(self.pool.clone()))
    }
}

fn submit<F, T>(jobs: &mpsc::Sender<Job>, future: F) -> Result<T, Error>
where
    F: Future<Output = Result<T, Error>> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    jobs.send(Box::pin(async move {
        let _ = tx.send(future.await);
    }))
    .map_err(|_| Error::Unavailable)?;
    rx.recv().map_err(|_| Error::Unavailable)?
}
