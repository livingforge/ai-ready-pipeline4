use anyhow::Result;
use arp4_cli::{
    data::{identifier, under},
    project,
};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct Options {
    /// Repository root; otherwise discover .arp/config.yml within the Git boundary.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Local execution to resume (not a document version).
    #[arg(long, default_value = "current")]
    run_id: String,
}

pub struct Paths {
    pub root: PathBuf,
    pub output: Option<PathBuf>,
}

impl Options {
    pub fn resolve(self) -> Result<Paths> {
        let root = match self.root {
            Some(root) => project::open(&root)?,
            None => project::discover(&std::env::current_dir()?)?,
        };
        project::open(&root)?;
        identifier(&self.run_id)?;
        Ok(Paths {
            root: under(&root, &format!(".arp/work/workflow/{}", self.run_id))?,
            output: Some(under(&root, ".arp/cache/output")?),
        })
    }
}
