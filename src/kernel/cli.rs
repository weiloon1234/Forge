use std::ffi::OsString;

use clap::Command;

use crate::cli::{CommandInvocation, CommandRegistry};
use crate::foundation::{AppContext, Result};

pub struct CliKernel {
    app: AppContext,
    registrars: Vec<crate::cli::CommandRegistrar>,
}

impl CliKernel {
    pub fn new(app: AppContext, registrars: Vec<crate::cli::CommandRegistrar>) -> Self {
        Self { app, registrars }
    }

    pub fn build_registry(&self) -> Result<CommandRegistry> {
        let mut registry = CommandRegistry::new();
        for registrar in &self.registrars {
            registrar(&mut registry)?;
        }
        Ok(registry)
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub async fn run(self) -> Result<()> {
        self.run_with_args(std::env::args_os()).await
    }

    pub async fn run_with_args<I, T>(self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let registry = self.build_registry()?;
        let mut root = Command::new("forge");
        for registered in registry.commands() {
            root = root.subcommand(registered.command.clone());
        }

        let matches = root
            .try_get_matches_from(args)
            .map_err(crate::foundation::Error::other)?;
        if let Some((name, sub_matches)) = matches.subcommand() {
            if let Some(registered) = registry
                .commands()
                .iter()
                .find(|command| command.id.as_str() == name)
            {
                (registered.handler)(CommandInvocation::new(
                    self.app.clone(),
                    sub_matches.clone(),
                ))
                .await?;
            }
        }

        Ok(())
    }
}
