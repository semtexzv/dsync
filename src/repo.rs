use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use futures::Stream;
use sha2::Digest;

pub struct Dir {
    pub id: String,
    pub name: String,
}

pub struct File {
    pub id: String,
    pub name: String,
    pub shasum: String,
    pub size: u64,
}

pub enum Entry {
    Dir(Dir),
    File(File),
}

impl Entry {
    fn name(&self) -> &str {
        match self {
            Entry::Dir(Dir { name, .. }) => name,
            Entry::File(File { name, .. }) => name,
        }
    }
}

pub trait FileSource {
    fn len(&self) -> impl Future<Output=usize>;

    fn stream(self, from: u64, chunks: usize) -> impl Stream<Item=Vec<u8>>;
}

pub trait Repo {
    async fn list(&self, path: PathBuf) -> anyhow::Result<Vec<Entry>>;
    async fn create_dir(&self, path: PathBuf) -> anyhow::Result<()>;
    async fn write_file(&self, path: PathBuf, data: impl FileSource) -> anyhow::Result<()>;
    async fn copy_file(&self, source: PathBuf, dest: PathBuf) -> anyhow::Result<()>;
    async fn delete(&self, path: PathBuf) -> anyhow::Result<()>;
}


pub struct LocalRepo {
    pub(crate) path: PathBuf,
}
fn shasum(file: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .open(file)?;

    let mut sha = sha2::Sha256::default();
    std::io::copy(&mut file, &mut sha)?;

    return Ok(hex::encode(sha.finalize()))
}

impl Repo for LocalRepo {
    async fn list(&self, path: PathBuf) -> anyhow::Result<Vec<Entry>> {
        let path = self.path.join(path);
        let entries = std::fs::read_dir(path)?;
        let mut out = vec![];
        for entry in entries {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                out.push(Entry::Dir(Dir {
                    id: entry.path().to_string_lossy().into_owned(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                }))
            } else if meta.is_file() {
                out.push(Entry::File(File {
                    id: entry.path().to_string_lossy().into_owned(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    shasum: shasum(&entry.path())?,
                    size: meta.len(),
                }))
            } else {
                panic!("Invalid file: {:?}", entry);
            }
        }
        return Ok(out)
    }

    async fn create_dir(&self, path: PathBuf) -> anyhow::Result<()> {
        let path = self.path.join(path);
        std::fs::create_dir_all(path)?;
        Ok(())
    }

    async fn write_file(&self, path: PathBuf, data: impl FileSource) -> anyhow::Result<()> {
        todo!()
    }

    async fn copy_file(&self, source: PathBuf, dest: PathBuf) -> anyhow::Result<()> {
        let src = self.path.join(source);
        let dst = self.path.join(dest);
        std::fs::copy(src, dst)?;
        Ok(())
    }

    async fn delete(&self, path: PathBuf) -> anyhow::Result<()> {
        let path = self.path.join(path);
        std::fs::remove_file(path)?;
        Ok(())
    }
}


pub async fn sync<S: Repo, D: Repo>(src: S, dst: D) -> anyhow::Result<()> {
    let root = PathBuf::from(".");

    let mut srcs = src.list(root.clone()).await?;
    let mut dsts = dst.list(root.clone()).await?;

    srcs.sort_by(|v1, v2| v1.name().cmp(v2.name()));

    let mut dsts: HashMap<&str, &Entry> = dsts.iter().map(|d| (d.name(), d)).collect();

    for entry in srcs {
        match entry {
            Entry::Dir(..) => {}
            Entry::File(src) => {
                match dsts.get(src.name.as_str()) {
                    Some(Entry::File(dst)) => {
                        if src.shasum == dst.shasum {
                            continue;
                        }
                    }
                    None => {}
                    _ => {}
                }
            }
        }
    }

    // For each entry in src,
    // If dir, Ensure DST exists in dir, sync dir

    // Get shasums of files in src,
    // For file in src, if shasum exists in dst, copy
    // If not, copy from src to dst
    Ok(())
}
