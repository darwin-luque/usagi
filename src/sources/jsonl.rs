use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

pub fn files(root: &Path, since: DateTime<Utc>) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    collect(root, since, &mut found)?;
    found.sort();
    Ok(found)
}

fn collect(dir: &Path, since: DateTime<Utc>, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            collect(&path, since, found)?;
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            let modified: DateTime<Utc> = entry.metadata()?.modified()?.into();
            if modified >= since {
                found.push(path);
            }
        }
    }
    Ok(())
}

pub fn lines(path: &Path, mut visit: impl FnMut(&str)) -> std::io::Result<()> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            return Ok(());
        }
        if let Ok(line) = std::str::from_utf8(&buffer) {
            visit(line.trim_end_matches(['\n', '\r']));
        }
    }
}
