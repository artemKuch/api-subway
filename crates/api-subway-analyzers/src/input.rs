use std::{fs, io, io::Read, path::Path};

#[derive(Debug)]
pub(crate) enum ReadTextError {
    Io(io::Error),
    Budget,
}

pub(crate) fn read_text_bounded(path: &Path, maximum_bytes: u64) -> Result<String, ReadTextError> {
    let mut file = fs::File::open(path).map_err(ReadTextError::Io)?;
    let metadata = file.metadata().map_err(ReadTextError::Io)?;
    if metadata.len() > maximum_bytes {
        return Err(ReadTextError::Budget);
    }
    let mut contents = String::new();
    file.by_ref()
        .take(maximum_bytes + 1)
        .read_to_string(&mut contents)
        .map_err(ReadTextError::Io)?;
    if contents.len() as u64 > maximum_bytes {
        return Err(ReadTextError::Budget);
    }
    Ok(contents)
}
