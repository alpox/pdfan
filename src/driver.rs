use std::{sync::Mutex, time::Duration};

use async_trait::async_trait;
use color_eyre::eyre::{Result, WrapErr};
use futures::future::join_all;
use tokio::{
    process::{Child, Command},
    select,
    sync::broadcast,
    task::JoinHandle,
    time,
};

#[async_trait]
pub trait Driver {
    type Proc: Process;
    async fn run(&self) -> Result<Self::Proc>;
}

#[async_trait]
pub trait Process {
    async fn stop(&mut self) -> Result<()>;
    async fn wait(&mut self) -> Result<()>;
}

#[async_trait]
impl Process for Child {
    async fn stop(&mut self) -> Result<()> {
        self.kill()
            .await
            .wrap_err("could not kill chromedriver process")
    }

    async fn wait(&mut self) -> Result<()> {
        self.wait()
            .await
            .map(|_| ())
            .wrap_err("could not wait for chromedriver process")
    }
}

pub struct ChromeDriver;

#[async_trait]
impl Driver for ChromeDriver {
    type Proc = Child;

    async fn run(&self) -> Result<Self::Proc> {
        let cmd = Command::new("chromedriver").args(["--port=4444"]).spawn();

        let proc = cmd.wrap_err("chromedriver exited")?;

        Ok(proc)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum SupervisorSignal {
    Interrupt,
}

pub struct Supervisor {
    tx: broadcast::Sender<SupervisorSignal>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            tx,
            tasks: Mutex::new(vec![]),
        }
    }

    pub fn run<T>(&self, task: T) -> &Self
    where
        T: Driver + Send + 'static,
        T::Proc: Send,
    {
        let mut rx = self.tx.subscribe();

        let handle = tokio::spawn(async move {
            let mut proc: Option<T::Proc> = task.run().await.ok();

            loop {
                select! {
                    Ok(SupervisorSignal::Interrupt) = rx.recv() => {
                        if let Some(mut p) = proc {
                            let _ = p.stop().await;
                        }
                        break;
                    },

                    // TODO: Log errors
                    _ = async {
                            match &mut proc {
                                Some(p) => p.wait().await,
                                None => {
                                    time::sleep(Duration::from_secs(1)).await;
                                    Ok(())
                                }
                            }
                        } => { proc = task.run().await.ok() }
                }
            }
        });

        self.tasks.lock().unwrap().push(handle);

        self
    }

    pub async fn stop(&self) {
        let _ = self.tx.send(SupervisorSignal::Interrupt);

        let handles = std::mem::take(&mut *self.tasks.lock().unwrap());
        join_all(handles).await;
    }
}
