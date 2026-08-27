use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::{Error, Result};

struct Entry {
    name: String,
    source_index: usize,
    directory: bool,
}

pub(crate) struct Archive<'a> {
    source: ZipArchive<Cursor<&'a [u8]>>,
    entries: Vec<Entry>,
    entry_indices: HashMap<String, usize>,
    changes: HashMap<String, Vec<u8>>,
    additions: Vec<String>,
    removals: HashSet<String>,
}

impl<'a> Archive<'a> {
    pub(crate) fn read(source: &'a [u8]) -> Result<Self> {
        let mut source = ZipArchive::new(Cursor::new(source))
            .map_err(|error| Error::InvalidArchive(error.to_string()))?;
        let mut entries = Vec::<Entry>::with_capacity(source.len());
        let mut entry_indices =
            HashMap::<String, usize>::with_capacity(source.len());
        for source_index in 0..source.len() {
            let file = source.by_index_raw(source_index)?;
            let name = file.name().to_owned();
            let directory = file.is_dir();
            drop(file);

            if let Some(&entry_index) = entry_indices.get(&name) {
                entries[entry_index].source_index = source_index;
                entries[entry_index].directory = directory;
            } else {
                entry_indices.insert(name.clone(), entries.len());
                entries.push(Entry { name, source_index, directory });
            }
        }
        Ok(Self {
            source,
            entries,
            entry_indices,
            changes: HashMap::new(),
            additions: Vec::new(),
            removals: HashSet::new(),
        })
    }

    pub(crate) fn get(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        if self.removals.contains(name) {
            return Ok(None);
        }
        if let Some(content) = self.changes.get(name) {
            return Ok(Some(content.clone()));
        }
        let Some(&entry_index) = self.entry_indices.get(name) else {
            return Ok(None);
        };
        let entry = &self.entries[entry_index];
        if entry.directory {
            return Ok(None);
        }

        let mut file = self.source.by_index(entry.source_index)?;
        let mut content = Vec::with_capacity(file.size().try_into().unwrap_or(0));
        file.read_to_end(&mut content)?;
        Ok(Some(content))
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        if self.removals.contains(name) {
            return false;
        }
        if self.changes.contains_key(name) {
            return true;
        }
        self.entry_indices
            .get(name)
            .is_some_and(|&index| !self.entries[index].directory)
    }

    pub(crate) fn set(
        &mut self,
        name: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) {
        let name = name.into();
        self.removals.remove(&name);
        if !self.entry_indices.contains_key(&name) && !self.additions.contains(&name)
        {
            self.additions.push(name.clone());
        }
        self.changes.insert(name, content.into());
    }

    pub(crate) fn remove(&mut self, name: &str) {
        self.changes.remove(name);
        self.removals.insert(name.to_owned());
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(|entry| !self.removals.contains(&entry.name))
            .map(|entry| entry.name.as_str())
            .chain(
                self.additions
                    .iter()
                    .filter(|name| !self.removals.contains(*name))
                    .map(String::as_str),
            )
    }

    pub(crate) fn finish(mut self) -> Result<Vec<u8>> {
        let output = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(output);
        writer.start_file(
            "mimetype",
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored),
        )?;
        writer.write_all(b"application/epub+zip")?;

        let compressed = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6));
        for entry in &self.entries {
            if entry.name == "mimetype" || self.removals.contains(&entry.name) {
                continue;
            }
            if let Some(content) = self.changes.remove(&entry.name) {
                writer.start_file(&entry.name, compressed)?;
                writer.write_all(&content)?;
            } else {
                let file = self.source.by_index_raw(entry.source_index)?;
                writer.raw_copy_file(file)?;
            }
        }
        for name in &self.additions {
            if name == "mimetype" || self.removals.contains(name) {
                continue;
            }
            let Some(content) = self.changes.remove(name) else {
                continue;
            };
            writer.start_file(name, compressed)?;
            writer.write_all(&content)?;
        }
        Ok(writer.finish()?.into_inner())
    }
}
