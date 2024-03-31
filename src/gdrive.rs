use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use anyhow::{bail, format_err};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::Stream;
use hyper::{Method, StatusCode};
use indexmap::IndexMap;
use oauth2::AccessToken;
use reqwest::header::{CONTENT_TYPE, HeaderMap};
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use tracing::info;
use crate::repo::{Dir, Entry, FileSource, Repo};

/// ref: https://developers.google.com/drive/api/reference/rest/v3/drives#Drive
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color_rgb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background_image_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capabilities: Option<DriveCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveCapabilities {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_page_token: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    drives: Vec<Drive>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_page_token: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<File>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    starred: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trashed: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    modified_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::serde_format::opt_string")]
    version: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    properties: Option<IndexMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app_properties: Option<IndexMap<String, serde_json::Value>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    drive_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file_extension: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    md5_checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    parents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thumbnail_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon_link: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256_checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha1_checksum: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::serde_format::opt_string")]
    size: Option<u64>,
}

pub struct RequestBuilder<API> {
    method: Method,
    path: String,
    query: IndexMap<&'static str, serde_json::Value>,
    body: Option<serde_json::Value>,
    headers: HeaderMap,

    _p: PhantomData<API>,
}

impl<API> Default for RequestBuilder<API> {
    fn default() -> Self {
        Self {
            method: Default::default(),
            path: "".to_string(),
            query: Default::default(),
            body: None,
            headers: Default::default(),
            _p: Default::default(),
        }
    }
}

pub trait APIMethod {
    type Response: DeserializeOwned;
}

pub trait APIListMethod {}

pub struct NoMethod;

const API_BASE: &str = "https://www.googleapis.com/drive/v3";

pub trait Authorizer {
    fn force_refresh(&self, client: &reqwest::Client) -> impl Future<Output=Result<AccessToken, anyhow::Error>>;
    fn token(&self, client: &reqwest::Client) -> impl Future<Output=Result<AccessToken, anyhow::Error>>;
}

impl<API: APIMethod> RequestBuilder<API> {
    pub fn fields(mut self, fields: impl Into<String>) -> Self {
        self.query.insert("fields", fields.into().into());
        self
    }

    pub async fn call<A: Authorizer>(mut self, client: &reqwest::Client, auth: &A) -> anyhow::Result<API::Response> {
        let path = &self.path;
        let mut force_refreshed = false;

        loop {
            let mut token = auth.token(client).await?;
            self.query.insert("access_token", token.secret().clone().into());

            let mut request = client
                .request(self.method.clone(), format!("{API_BASE}/{path}"))
                .headers(self.headers.clone())
                .query(&self.query);

            if let Some(body) = &self.body {
                request = request
                    .json(&body)
                    .header(CONTENT_TYPE, "application/json")
            }

            let response = request
                .send()
                .await?;

            info!("Response: {response:?}");

            if response.status() == StatusCode::UNAUTHORIZED && !force_refreshed {
                token = auth.force_refresh(client).await?;
                force_refreshed = true;
            } else if force_refreshed {
                bail!("Probably invalid account, investigate")
            } else {
                return Ok(response.json().await?);
            }
        }
    }
}


impl<API: APIListMethod> RequestBuilder<API> {
    pub fn page_size(mut self, size: i32) -> Self {
        self.query.insert("pageSize", size.into());
        self
    }

    pub fn page_token(mut self, token: impl Into<String>) -> Self {
        self.query.insert("pageToken", token.into().into());
        self
    }

    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query.insert("q", query.into().into());
        self
    }
}

pub fn builder() -> RequestBuilder<NoMethod> {
    RequestBuilder::default()
}

impl RequestBuilder<NoMethod> {
    pub fn files_get(self, id: impl Into<String>) -> RequestBuilder<GetFile> {
        RequestBuilder {
            method: Method::GET,
            path: format!("files/{id}", id = id.into()),
            query: self.query,
            ..Default::default()
        }
    }
    pub fn files_create(self, file: File) -> RequestBuilder<CreateFile> {
        RequestBuilder {
            method: Method::POST,
            path: "files".to_string(),
            query: self.query,
            body: Some(serde_json::to_value(file).unwrap()),
            ..Default::default()
        }
    }
    pub fn files_copy(self, id: String, to: File) -> RequestBuilder<CopyFile> {
        RequestBuilder {
            method: Method::POST,
            path: format!("files/{id}/copy"),
            query: self.query,
            body: Some(serde_json::to_value(to).unwrap()),
            ..Default::default()
        }
    }
    pub fn files_delete(self, id: String) -> RequestBuilder<CopyFile> {
        RequestBuilder {
            method: Method::DELETE,
            path: format!("files/{id}"),
            query: self.query,
            ..Default::default()
        }
    }
    pub fn trash_empty(self) -> RequestBuilder<CopyFile> {
        RequestBuilder {
            method: Method::DELETE,
            path: "files/trash".to_string(),
            query: self.query,
            ..Default::default()
        }
    }

    pub fn drives_list(self) -> RequestBuilder<ListDrives> {
        RequestBuilder {
            method: Method::GET,
            path: "drives".to_string(),
            query: self.query,
            ..Default::default()
        }
    }
    pub fn files_list(self) -> RequestBuilder<ListFiles> {
        RequestBuilder {
            method: Method::GET,
            path: "files".to_string(),
            query: self.query,
            ..Default::default()
        }
    }
}

pub struct ListDrives;

impl APIMethod for ListDrives {
    type Response = DriveList;
}

impl APIListMethod for ListDrives {}


pub struct ListFiles;

impl APIMethod for ListFiles {
    type Response = FileList;
}

impl APIListMethod for ListFiles {}

pub struct GetFile;

impl APIMethod for GetFile {
    type Response = File;
}

pub struct CreateFile;

impl APIMethod for CreateFile {
    type Response = File;
}

pub struct CopyFile;

impl APIMethod for CopyFile {
    type Response = File;
}

pub struct GDriveRepo<A: Authorizer> {
    auth: A,
    root_id: String,
    /// Directory tree
    dirs: DashMap<PathBuf, String>,
    fils: DashMap<PathBuf, Vec<String>>,
    client: reqwest::Client,
}

impl<A: Authorizer> GDriveRepo<A> {
    pub async fn new(client: &reqwest::Client, auth: A) -> anyhow::Result<Self> {
        let root: File = builder()
            .files_get("root")
            .fields("id, name")
            .call(&client, &auth)
            .await?;

        let root_id = root.id.as_deref().unwrap().to_owned();

        let folders: FileList = builder()
            .files_list()
            .fields("files(id, size, name, parents)")
            .query("mimeType = 'application/vnd.google-apps.folder'")
            .call(&client, &auth)
            .await?;

        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut parents: HashMap<&str, &str> = HashMap::new();
        let mut names: HashMap<&str, &str> = HashMap::new();

        folders.files
            .iter()
            .for_each(|file| {
                let par = file.parents.first().unwrap().as_str();
                let chl = file.id.as_deref().unwrap();

                children.entry(par)
                    .or_default()
                    .push(chl);

                parents.insert(chl, par);
                names.insert(file.id.as_deref().unwrap(), file.name.as_deref().unwrap());
            });

        let mut paths = HashMap::<&str, PathBuf>::new();

        paths.insert(&root_id, PathBuf::from("/"));

        fn add_child<'a>(
            id: &'a str,
            parent: &'a str,
            names: &HashMap<&'a str, &'a str>,
            children: &HashMap<&'a str, Vec<&'a str>>,
            paths: &mut HashMap<&'a str, PathBuf>,
        ) {
            if paths.contains_key(id) {
                return;
            }

            let par = paths.get(parent).cloned().unwrap();
            paths.insert(id, par.join(names[id]));

            if let Some(cld) = children.get(id) {
                for cld in cld {
                    add_child(cld, id, names, children, paths)
                }
            }
        }
        if let Some(dirs) = children.get(root_id.as_str()) {
            for dir in dirs {
                add_child(dir, root_id.as_str(), &names, &children, &mut paths);
            }
        }
        let dirs = paths
            .into_iter()
            .map(|(id, path)| (path, id.to_string()))
            .collect();

        Ok(Self {
            auth,
            root_id,
            dirs,
            fils: Default::default(),
            client: client.clone(),
        })
    }
}

impl<A: Authorizer> Repo for GDriveRepo<A> {
    async fn list(&self, path: PathBuf) -> anyhow::Result<Vec<Entry>> {
        let path = PathBuf::from("/").join(path);
        let dir_id = self.dirs.get(&path)
            .ok_or_else(|| format_err!("Missing dir: {path:?}"))?.clone();

        let mut page_token: Option<String> = None;
        let mut files = vec![];

        loop {
            let mut req = builder()
                .files_list()
                .page_size(1000);

            if let Some(page_token) = page_token {
                req = req.page_token(page_token)
            }

            let mut file_page: FileList = req
                .query(format!("'{}' in parents and trashed = false", dir_id))
                .fields("files(id, name, size, sha256Checksum, mimeType, trashed)")
                .call(&self.client, &self.auth)
                .await?;

            page_token = file_page.next_page_token;
            files.append(&mut file_page.files);

            if page_token.is_none() {
                break;
            }
        }

        return Ok(files
            .into_iter()
            .map(|file| {
                println!("{:#?}", file);
                if file.mime_type.as_deref() == Some("application/vnd.google-apps.folder") {
                    Entry::Dir(crate::repo::Dir {
                        id: file.id.unwrap(),
                        name: file.name.unwrap(),
                    })
                } else {
                    Entry::File(crate::repo::File {
                        id: file.id.unwrap(),
                        name: file.name.unwrap(),
                        shasum: file.sha256_checksum.unwrap(),
                        size: file.size.unwrap(),
                    })
                }
            })
            .collect());
    }

    async fn create_dir(&self, path: PathBuf) -> anyhow::Result<()> {
        if let Some(path) = self.dirs.get(&path) {
            return Ok(());
        };
        let future = Box::pin(self.create_dir(path.parent().unwrap().to_owned()));
        future.await?;

        let parent = self.dirs.get(path.parent().unwrap()).unwrap().clone();

        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let file = File {
            name: Some(name),
            mime_type: Some("application/vnd.google-apps.folder".to_string()),
            parents: vec![parent],
            ..Default::default()
        };

        let file: File = builder()
            .files_create(file)
            .fields("id, name")
            .call(&self.client, &self.auth)
            .await?;

        self.dirs.insert(path, file.id.unwrap());

        info!("DIRS: {:#?}", self.dirs);

        return Ok(());
    }

    async fn write_file(&self, path: PathBuf, data: impl FileSource) -> anyhow::Result<()> {
        let len = data.len().await;

        // let file = File {
        //
        // }
        todo!()
    }

    async fn copy_file(&self, source: PathBuf, dest: PathBuf) -> anyhow::Result<()> {
        let sdir = source.parent().unwrap();
        let sdir = self.dirs.get(sdir).unwrap().clone();
        let sname = source.file_name().unwrap();

        let ddir = dest.parent().unwrap();
        let ddir = self.dirs.get(ddir).unwrap().clone();
        let dname = dest.file_name().unwrap().to_string_lossy().to_string();

        let mut files: FileList = builder()
            .files_list()
            .fields("files(id, name, size, sha256Checksum)")
            .query(format!("name = '{}' and '{}' in parents", sname.to_string_lossy(), sdir))
            .call(&self.client, &self.auth)
            .await?;

        let target = File {
            name: Some(dname),
            parents: vec![ddir],
            ..Default::default()
        };

        let first = files.files.remove(0);

        builder()
            .files_copy(first.id.unwrap(), target)
            .call(&self.client, &self.auth)
            .await?;

        Ok(())
    }

    async fn delete(&self, path: PathBuf) -> anyhow::Result<()> {

        todo!()
    }
}