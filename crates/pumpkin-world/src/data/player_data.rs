use pumpkin_nbt::compound::NbtCompound;
use std::fs::{File, create_dir_all};
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, error};
use uuid::Uuid;

/// Manages the storage and retrieval of player data from disk and memory cache.
///
/// This struct provides functions to load and save player data to/from NBT files,
/// with a memory cache to handle player disconnections temporarily.
pub struct PlayerDataStorage {
    /// Path to the directory where player data is stored
    data_path: PathBuf,
    /// Whether player data saving is enabled
    save_enabled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PlayerDataError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("NBT error: {0}")]
    Nbt(String),
}

impl PlayerDataStorage {
    /// Creates a new `PlayerDataStorage` with the specified data path and cache expiration time.
    pub fn new(data_path: impl Into<PathBuf>, enabled: bool) -> Self {
        let path = data_path.into();
        if !path.exists()
            && let Err(e) = create_dir_all(&path)
        {
            error!(
                "Failed to create player data directory at {}: {e}",
                path.display()
            );
        }

        Self {
            data_path: path,
            save_enabled: enabled,
        }
    }

    #[must_use]
    pub const fn get_data_path(&self) -> &PathBuf {
        &self.data_path
    }

    #[must_use]
    pub const fn is_save_enabled(&self) -> bool {
        self.save_enabled
    }

    pub const fn set_save_enabled(&mut self, enabled: bool) {
        self.save_enabled = enabled;
    }

    /// Returns the path for a player's data file based on their UUID.
    #[must_use]
    pub fn get_player_data_path(&self, uuid: &Uuid) -> PathBuf {
        self.get_data_path().join(format!("{uuid}.dat"))
    }

    /// Loads player data from NBT file or cache.
    ///
    /// This function first checks if player data exists in the cache.
    /// If not, it attempts to load the data from a .dat file on disk.
    ///
    /// # Arguments
    ///
    /// * `uuid` - The UUID of the player to load data for.
    ///
    /// # Returns
    ///
    /// A Result containing either the player's NBT data or an error.
    pub fn load_player_data(&self, uuid: &Uuid) -> Result<(bool, NbtCompound), PlayerDataError> {
        // If player data saving is disabled, return empty data
        if !self.is_save_enabled() {
            return Ok((false, NbtCompound::new()));
        }

        // If not in cache, load from disk
        let path = self.get_player_data_path(uuid);
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                debug!("No player data file found for {uuid}");
                return Ok((false, NbtCompound::new()));
            }
            Err(e) => {
                error!("Failed to open player data file for {uuid}: {e}");
                return Err(PlayerDataError::Io(e));
            }
        };

        match pumpkin_nbt::nbt_compress::read_gzip_compound_tag(file) {
            Ok(nbt) => {
                debug!("Loaded player data for {uuid} from disk");
                Ok((true, nbt))
            }
            Err(e) => {
                error!("Failed to read player data for {uuid}: {e}");
                Err(PlayerDataError::Nbt(e.to_string()))
            }
        }
    }

    /// Saves player data to NBT file and updates cache.
    ///
    /// This function saves the player's data to a .dat file on disk and also
    /// updates the in-memory cache with the latest data.
    ///
    /// # Arguments
    ///
    /// * `uuid` - The UUID of the player to save data for.
    /// * `data` - The NBT compound data to save.
    ///
    /// # Returns
    ///
    /// A Result indicating success or the error that occurred.
    pub fn save_player_data(&self, uuid: &Uuid, data: NbtCompound) -> Result<(), PlayerDataError> {
        // Skip saving if disabled in config
        if !self.is_save_enabled() {
            return Ok(());
        }

        let path = self.get_player_data_path(uuid);

        let result = write_atomic(&path, |file| {
            pumpkin_nbt::nbt_compress::write_gzip_compound_tag(data, file)
                .map_err(|e| PlayerDataError::Nbt(e.to_string()))
        });
        match &result {
            Ok(()) => debug!("Saved player data for {uuid} to disk"),
            Err(e) => error!("Failed to save player data for {uuid}: {e}"),
        }
        result
    }
}

fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<(), PlayerDataError>,
) -> Result<(), PlayerDataError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    write(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;

    // Persist in the same directory so readers see either the old or the new file.
    temporary.persist(path).map_err(|err| err.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn failed_write_preserves_previous_save() -> Result<(), PlayerDataError> {
        let directory = tempfile::tempdir()?;
        let storage = PlayerDataStorage::new(directory.path(), true);
        let uuid = Uuid::nil();
        let mut original = NbtCompound::new();
        original.put_int("XpLevel", 12);
        storage.save_player_data(&uuid, original)?;

        let path = storage.get_player_data_path(&uuid);
        let result = write_atomic(&path, |file| {
            file.write_all(b"incomplete")?;
            Err(io::Error::other("injected write failure").into())
        });

        assert!(result.is_err());
        let (loaded, data) = storage.load_player_data(&uuid)?;
        assert!(loaded);
        assert_eq!(data.get_int("XpLevel"), Some(12));
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn atomic_save_replaces_existing_player_data() -> Result<(), PlayerDataError> {
        let directory = tempfile::tempdir()?;
        let storage = PlayerDataStorage::new(directory.path(), true);
        let uuid = Uuid::nil();

        for level in [12, 34] {
            let mut data = NbtCompound::new();
            data.put_int("XpLevel", level);
            storage.save_player_data(&uuid, data)?;
        }

        let (loaded, data) = storage.load_player_data(&uuid)?;
        assert!(loaded);
        assert_eq!(data.get_int("XpLevel"), Some(34));
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }
}
