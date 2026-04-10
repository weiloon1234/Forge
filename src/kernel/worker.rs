use crate::foundation::{AppContext, Result};
use crate::jobs::Worker;

pub struct WorkerKernel {
    worker: Worker,
}

impl WorkerKernel {
    pub fn new(app: AppContext) -> Result<Self> {
        Ok(Self {
            worker: Worker::from_app(app)?,
        })
    }

    pub fn app(&self) -> &AppContext {
        self.worker.app()
    }

    pub async fn run(self) -> Result<()> {
        self.worker.run().await
    }

    pub async fn run_once(&self) -> Result<bool> {
        self.worker.run_once().await
    }
}
