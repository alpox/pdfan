use color_eyre::eyre::{Context, Result};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::{
    OwnedSemaphorePermit, Semaphore,
    oneshot::{self},
};

pub trait Task<Ctx> {
    type Result;
    fn process(&self, ctx: &mut Ctx) -> impl Future<Output = Self::Result> + std::marker::Send;
}

struct Packet<Ctx, T: Task<Ctx>> {
    task: T,
    tx: oneshot::Sender<T::Result>,
    _permit: OwnedSemaphorePermit,
}

impl<Ctx, T: Task<Ctx>> Packet<Ctx, T> {
    fn new(task: T, tx: oneshot::Sender<T::Result>, permit: OwnedSemaphorePermit) -> Self {
        Self {
            task,
            tx,
            _permit: permit,
        }
    }

    fn send(self, result: T::Result) -> Result<(), T::Result> {
        self.tx.send(result)
    }
}

pub struct WorkerPool<Ctx, T: Task<Ctx>> {
    tx: async_channel::Sender<Packet<Ctx, T>>,
    semaphore: Arc<Semaphore>,
}

impl<T, Ctx> WorkerPool<Ctx, T>
where
    T: Task<Ctx> + Send + Sync + 'static,
    T::Result: Send + 'static,
    Ctx: Send + 'static,
{
    pub fn new<F, Fut>(cap: usize, workers: usize, make_ctx: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + Clone + 'static,
        Fut: Future<Output = Result<Ctx>> + Send + 'static,
    {
        let semaphore = Arc::new(Semaphore::new(cap));
        let (tx, rx) = async_channel::unbounded();

        for _ in 0..workers {
            let rx = rx.clone();
            let make_ctx = make_ctx.clone();
            tokio::spawn(spawn_worker(rx, make_ctx));
        }

        Self { tx, semaphore }
    }

    pub async fn queue(&self, task: T, timeout: Duration) -> Result<T::Result> {
        tokio::time::timeout(timeout, async {
            let permit = self
                .semaphore
                .clone()
                .acquire_owned()
                .await
                .wrap_err("Pool shut down")?;

            let (tx, rx) = oneshot::channel();
            let packet = Packet::new(task, tx, permit);

            self.tx
                .send(packet)
                .await
                .wrap_err("Could not send to worker queue")?;
            rx.await.wrap_err("Worker dropped")
        })
        .await
        .wrap_err("Task timed out")?
    }
}

async fn spawn_worker<T, Ctx, F, Fut>(rx: async_channel::Receiver<Packet<Ctx, T>>, make_ctx: F)
where
    T: Task<Ctx>,
    F: Fn() -> Fut,
    Fut: Future<Output = Result<Ctx>>,
{
    let mut ctx = make_ctx().await.unwrap();

    while let Ok(packet) = rx.recv().await {
        if packet.tx.is_closed() {
            continue;
        }
        let result = packet.task.process(&mut ctx).await;
        let _ = packet.send(result);
    }
}
