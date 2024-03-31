mod auth;
mod gdrive;
mod serde_format;
mod cli;
mod repo;

use crate::gdrive::{Authorizer, builder, GDriveRepo};
use clap::Parser;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::{Future, ready, Ready};
use std::io::{Seek, SeekFrom};
use std::ops::Add;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;
use anyhow::{bail, Error};
use futures::Stream;
use indexmap::IndexMap;
use oauth2::{AccessToken, RefreshToken, Scope, TokenResponse};
use tracing::warn;
use crate::cli::Args;
use crate::repo::{LocalRepo, Repo, sync};

static LOCK: Mutex<()> = Mutex::new(());

pub fn get<T: Serialize + DeserializeOwned>(name: &str) -> Option<T> {
    let _lck = LOCK.lock().unwrap();

    let path = dirs::config_local_dir().unwrap().join(".dsync");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .unwrap();

    let cfg: BTreeMap<String, serde_json::Value> = serde_json::from_reader(file).ok()?;

    cfg.get(name)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub fn set<T: Serialize + DeserializeOwned>(name: &str, v: &T) {
    let _lck = LOCK.lock().unwrap();

    let path = dirs::config_local_dir().unwrap().join(".dsync");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .unwrap();

    let mut cfg: BTreeMap<String, serde_json::Value> =
        serde_json::from_reader(&file).unwrap_or_default();

    cfg.insert(name.to_string(), serde_json::to_value(v).unwrap());

    file.seek(SeekFrom::Start(0)).unwrap();
    serde_json::to_writer_pretty(file, &cfg).unwrap()
}

pub fn with<T: Default + Serialize + DeserializeOwned, R>(name: &str, fun: impl FnOnce(&mut T) -> R) -> R {
    let _lck = LOCK.lock().unwrap();

    let path = dirs::config_local_dir().unwrap().join(".dsync");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .unwrap();

    let mut cfg: BTreeMap<String, serde_json::Value> =
        serde_json::from_reader(&file).unwrap_or_default();

    let item = cfg.entry(name.to_string())
        .or_insert_with(|| serde_json::to_value(T::default()).unwrap());

    let mut item: T = serde_json::from_value(item.clone()).unwrap();

    let out = fun(&mut item);

    file.seek(SeekFrom::Start(0)).unwrap();
    serde_json::to_writer_pretty(file, &cfg).unwrap();
    drop(_lck);
    return out;
}

pub const DRIVES: &str = "drives";

pub type Drives = IndexMap<String, DriveInfo>;

#[derive(Debug, Serialize, Deserialize)]
struct DriveInfo {
    access_token: AccessToken,
    access_until: SystemTime,

    refresh_token: RefreshToken,
    scopes: Vec<Scope>,
}

struct GDriveAuthorizer {
    name: String,
    lock: tokio::sync::Mutex<()>,
}

impl Authorizer for GDriveAuthorizer {
    fn force_refresh(&self, client: &reqwest::Client) -> impl Future<Output=Result<AccessToken, Error>> {
        async move {
            let lock = self.lock.lock().await;

            let mut drives = get::<Drives>(DRIVES).unwrap_or_default();
            let mut drive = drives.get_mut(&self.name)
                .unwrap();

            let (valid_until, response) = crate::auth::refresh(client, &drive.refresh_token).await?;

            drive.access_token = response.access_token().clone();
            drive.access_until = valid_until;

            set(DRIVES, &drives);

            drop(lock);

            Ok(response.access_token().clone())
        }
    }

    fn token(&self, client: &reqwest::Client) -> impl Future<Output=Result<AccessToken, Error>> {
        let mut drives = get::<Drives>(DRIVES).unwrap_or_default();
        async move {
            let mut drive = drives.get_mut(&self.name)
                .unwrap();

            return if drive.access_until > SystemTime::now() {
                Ok(drive.access_token.clone())
            } else {
                warn!("Token expired, refreshing");
                self.force_refresh(client).await
            };
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();
    std::env::set_var("RUST_LOG", "trace");
    tracing_subscriber::fmt().init();

    let client = reqwest::ClientBuilder::new()
        .gzip(true)
        .build()?;

    match args {
        Args::Drive(cli::Drive::List) => {
            let drives = get::<Drives>(DRIVES).unwrap();
            println!("These are the drives you have: ");
            drives.iter().for_each(|(name, drive)| {
                println!("{name}: {drive:?}")
            });
            return Ok(());
        }
        Args::Drive(cli::Drive::Show { name }) => {
            let drives = get::<Drives>(DRIVES)
                .unwrap_or_default();
            if let Some(drive) = drives.get(&name) {
                println!("{drive:?}");
            } else {
                println!("Drive not found");
            }
            return Ok(());
        }
        Args::Drive(cli::Drive::Add { name, code }) => {
            let mut old = get::<IndexMap<String, DriveInfo>>(DRIVES).unwrap_or_default();
            if let Some(old) = old.get(&name) {
                bail!("Drive already exists: {old:?}");
            }
            let (valid_until, response) = crate::auth::auth(&client).await?;
            let drive = DriveInfo {
                access_token: response.access_token().clone(),
                access_until: valid_until,
                refresh_token: response.refresh_token().unwrap().clone(),
                scopes: response.scopes().map(Clone::clone).unwrap_or_default(),
            };
            old.insert(name, drive);
            set(DRIVES, &old);
            return Ok(());
        }
        Args::Drive(cli::Drive::Rm { name }) => {
            let mut old = get::<IndexMap<String, DriveInfo>>(DRIVES).unwrap_or_default();
            old.shift_remove(&name);
            set(DRIVES, &old);
            return Ok(());
        }
        Args::Sync(cli::Sync { src, dst }) => {
            println!("{src:?} to {dst:?}");

            match (src.prefix, dst.prefix) {
                (Some(drive), None) => {
                    let auth = GDriveAuthorizer { name: drive, lock: Default::default() };

                    let srepo = GDriveRepo::new(&client, auth).await?;
                    let drepo = LocalRepo { path: dst.path.canonicalize().unwrap() };


                    sync(srepo, drepo).await?
                }
                (None, Some(drive)) => {
                    let auth = GDriveAuthorizer { name: drive, lock: Default::default() };

                    let srepo = LocalRepo { path: src.path.canonicalize().unwrap() };
                    let drepo = GDriveRepo::new(&client, auth).await?;


                    drepo.list(PathBuf::from("/media/2020-04-30")).await.unwrap();
                    drepo.create_dir(PathBuf::from("/aaa")).await.unwrap();

                    sync(srepo, drepo).await?
                }
                _ => {
                    panic!("Exactly one location must have <drive>: prefix")
                }
            }
            return Ok(());
        }
    }

    // let root = builder()
    //     .file_get("root")
    //     .fields("id, name")
    //     // .query("mimeType = 'application/vnd.google-apps.folder' and 'root' in parents")
    //     .access_token(token.access_token().secret())
    //     .call(&client)
    //     .await?;

    // let folders = builder()
    //     .files_list()
    //     .fields("files(id, size, name, sha256Checksum, parents)")
    //     .query("mimeType = 'application/vnd.google-apps.folder' and 'root' in parents")
    //     .access_token(token.access_token().secret())
    //     .call(&client)
    //     .await?;
    //
    // let files = builder()
    //     .files_list()
    //     .fields("files(id, size, name, sha256Checksum, parents)")
    //     .access_token(token.access_token().secret())
    //     .call(&client)
    //     .await?;

    // println!("{:#?}", root);

    // let out = client.request(Method::POST, format!("https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable"))
    //     .query(&json!({"fields": "maxUploadSize"}))
    //     .header(AUTHORIZATION, format!("Bearer {}", token.access_token().secret()))
    //     .send()
    //     .await
    //     .unwrap()
    //     .json::<serde_json::Value>()
    //     .await
    //     .unwrap();

    Ok(())

    // let client = reqwest::ClientBuilder::new().build()?;
    // match args {
    //     Args::Copy(Copy { src, dst }) => {}
    // }
    //
    // Ok(())
}
