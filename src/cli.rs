use std::path::PathBuf;
use std::str::FromStr;
use clap::{Parser, Subcommand};
use serde_json::to_string;

#[derive(Debug, Clone)]
pub struct PrefixedPath {
    pub prefix: Option<String>,
    pub path: PathBuf,
}

impl FromStr for PrefixedPath {
    type Err = core::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let pos = s.find(":");
        if let Some(pos) = pos {
            return Ok(Self {
                prefix: Some(s[..pos].to_string()),
                path: FromStr::from_str(&s[pos + 1..])?,
            });
        } else {
            return Ok(Self {
                prefix: None,
                path: FromStr::from_str(&s)?,
            });
        }
    }
}

#[derive(Debug, Parser)]
pub struct Sync {
    #[arg(name = "src", help = "Source path")]
    pub src: PrefixedPath,
    #[arg(name = "dst", help = "Destination path")]
    pub dst: PrefixedPath,
}

#[derive(Debug, Parser)]
pub enum Drive {
    #[command(name = "list", alias = "ls", about = "List all drives")]
    List,
    #[command(name = "show", about = "Show info about a drive")]
    Show {
        #[arg(name = "name", required = true, help = "Name of the drive")]
        name: String,
    },
    #[command(name = "add", alias = "ad", about = "Connect google drive")]
    Add {
        #[arg(name = "name", required = true, help = "Name of the repo to create")]
        name: String,
        #[arg(name = "code", short, long, help = "Use code instead of browser to sign-in")]
        code: bool,
    },
    #[command(name = "rm", alias = "del", about = "Disconnect google drive")]
    Rm {
        #[arg(name = "name", required = true, help = "Name of the repo to create")]
        name: String,
    },
}

#[derive(Debug, Parser)]
pub enum Args {
    #[command(name = "sync")]
    Sync(Sync),
    #[command(subcommand, name = "drive")]
    Drive(Drive),
}
