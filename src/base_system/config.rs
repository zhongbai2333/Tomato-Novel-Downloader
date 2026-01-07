//! 配置文件读写与带注释生成。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_yaml::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid yaml at {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    #[error("validation error: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, Copy)]
pub struct FieldMeta {
    pub name: &'static str,
    pub description: &'static str,
}

pub trait ConfigSpec: Serialize + DeserializeOwned + Default {
    const FILE_NAME: &'static str;
    fn fields() -> &'static [FieldMeta];
}

pub fn load_or_create<T: ConfigSpec>(config_path: Option<&Path>) -> Result<T, ConfigError> {
    load_or_create_with_base::<T>(config_path, None)
}

/// Load or create a config file, optionally using a base directory.
///
/// # Arguments
/// * `config_path` - If provided, uses this exact path (base_dir is ignored)
/// * `base_dir` - If provided and config_path is None, creates config in base_dir
///
/// # Path resolution
/// - If config_path is Some: uses the exact path provided
/// - If config_path is None and base_dir is Some: uses base_dir/FILE_NAME
/// - If both are None: uses current directory/FILE_NAME
pub fn load_or_create_with_base<T: ConfigSpec>(
    config_path: Option<&Path>,
    base_dir: Option<&Path>,
) -> Result<T, ConfigError> {
    let path = resolve_path::<T>(config_path, base_dir);
    ensure_parent(&path)?;

    if !path.exists() {
        let default_config = T::default();
        write_with_comments(&default_config, &path)?;
        return Ok(default_config);
    }

    let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;

    let user_yaml: Value = serde_yaml::from_str(&raw).map_err(|source| ConfigError::Parse {
        path: path.clone(),
        source,
    })?;

    let mut merged = serde_yaml::to_value(T::default())
        .map_err(|err| ConfigError::Validation(err.to_string()))?;
    merge_values(&mut merged, user_yaml);

    let config: T =
        serde_yaml::from_value(merged).map_err(|err| ConfigError::Validation(err.to_string()))?;

    if has_missing_fields::<T>(&path)? {
        write_with_comments(&config, &path)?;
    }

    Ok(config)
}

pub fn write_with_comments<T: ConfigSpec>(config: &T, path: &Path) -> Result<(), ConfigError> {
    ensure_parent(path)?;
    let yaml = generate_yaml_with_comments(config)?;
    fs::write(path, yaml).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn generate_yaml_with_comments<T: ConfigSpec>(config: &T) -> Result<String, ConfigError> {
    let value =
        serde_yaml::to_value(config).map_err(|err| ConfigError::Validation(err.to_string()))?;
    let mapping = match value {
        Value::Mapping(map) => map,
        _ => {
            return Err(ConfigError::Validation(
                "config must serialize to a mapping".to_string(),
            ));
        }
    };

    let mut lines = Vec::new();
    for field in T::fields() {
        if !field.description.is_empty() {
            lines.push(format!("# {}", field.description.replace('\n', "\n# ")));
        }
        let key = Value::String(field.name.to_string());
        let val = mapping.get(&key).cloned().unwrap_or(Value::Null);
        let yaml_line = serde_yaml::to_string(&serde_yaml::Mapping::from_iter([(key, val)]))
            .map_err(|err| ConfigError::Validation(err.to_string()))?;
        lines.push(yaml_line.trim().to_string());
    }

    Ok(lines.join("\n"))
}

fn has_missing_fields<T: ConfigSpec>(path: &Path) -> Result<bool, ConfigError> {
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let user_yaml: Value = serde_yaml::from_str(&raw).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    let Value::Mapping(map) = user_yaml else {
        return Ok(true);
    };
    for field in T::fields() {
        if !map.contains_key(Value::String(field.name.to_string())) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn merge_values(default: &mut Value, user: Value) {
    match (default, user) {
        (Value::Mapping(dest), Value::Mapping(src)) => {
            for (key, user_val) in src {
                if let Some(dest_val) = dest.get_mut(&key) {
                    merge_values(dest_val, user_val);
                } else {
                    dest.insert(key, user_val);
                }
            }
        }
        (dest, Value::Sequence(src_seq)) => {
            *dest = Value::Sequence(src_seq);
        }
        (dest, Value::Tagged(tagged)) => {
            *dest = Value::Tagged(tagged);
        }
        (dest, other) => {
            *dest = other;
        }
    }
}

fn resolve_path<T: ConfigSpec>(path: Option<&Path>, base_dir: Option<&Path>) -> PathBuf {
    if let Some(p) = path {
        p.to_path_buf()
    } else if let Some(base) = base_dir {
        base.join(T::FILE_NAME)
    } else {
        PathBuf::from(T::FILE_NAME)
    }
}

fn ensure_parent(path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}
