use std::{
    collections::BTreeMap,
    env::{self, VarError},
    ffi::OsString,
    path::PathBuf,
};

pub(super) trait EnvSource {
    fn var(&self, name: &str) -> Result<Option<String>, VarError>;
    fn var_os(&self, name: &str) -> Option<OsString>;
}

pub(super) struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var(&self, name: &str) -> Result<Option<String>, VarError> {
        match env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(VarError::NotPresent) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn var_os(&self, name: &str) -> Option<OsString> {
        env::var_os(name)
    }
}

impl EnvSource for BTreeMap<String, String> {
    fn var(&self, name: &str) -> Result<Option<String>, VarError> {
        Ok(self.get(name).cloned())
    }

    fn var_os(&self, name: &str) -> Option<OsString> {
        self.get(name).map(OsString::from)
    }
}

pub(super) struct EnvReader<'a, S> {
    source: &'a S,
}

impl<'a, S: EnvSource> EnvReader<'a, S> {
    pub(super) fn new(source: &'a S) -> Self {
        Self { source }
    }

    pub(super) fn lookup(&self, name: &str) -> Result<Option<String>, VarError> {
        self.source.var(name)
    }

    pub(super) fn read_bounded_size(
        &self,
        name: &str,
        default: usize,
        maximum: usize,
    ) -> Result<usize, String> {
        let Some(value) = self.string(name)? else {
            return Ok(default);
        };
        let parsed = value
            .parse::<usize>()
            .map_err(|_| format!("{name} must be an integer between 1 and {maximum}"))?;
        if !(1..=maximum).contains(&parsed) {
            return Err(format!("{name} must be an integer between 1 and {maximum}"));
        }
        Ok(parsed)
    }

    pub(super) fn read_positive_u64(&self, name: &str, default: u64) -> Result<u64, String> {
        let Some(value) = self.string(name)? else {
            return Ok(default);
        };
        parse_positive_u64(name, &value)
    }

    pub(super) fn read_u64_list(&self, name: &str, default: &[u64]) -> Result<Vec<u64>, String> {
        let Some(value) = self.string(name)? else {
            return Ok(default.to_vec());
        };
        let values: Vec<_> = value
            .split(',')
            .map(str::trim)
            .map(|item| {
                item.parse::<u64>()
                    .map_err(|_| format!("{name} must be a comma-separated list of u64 seeds"))
            })
            .collect::<Result<_, _>>()?;
        if values.is_empty() {
            return Err(format!("{name} must contain at least one seed"));
        }
        Ok(values)
    }

    pub(super) fn read_bool(&self, name: &str, default: bool) -> Result<bool, String> {
        let Some(value) = self.string(name)? else {
            return Ok(default);
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!(
                "{name} must be one of true/false, 1/0, yes/no, or on/off"
            )),
        }
    }

    pub(super) fn read_f64(
        &self,
        name: &str,
        default: f64,
        minimum_exclusive: f64,
        maximum: f64,
    ) -> Result<f64, String> {
        let value = match self.string(name)? {
            Some(raw) => raw
                .parse::<f64>()
                .map_err(|error| format!("{name} must be a number: {error}"))?,
            None => default,
        };
        if !value.is_finite() || value <= minimum_exclusive || value > maximum {
            return Err(format!(
                "{name} must be finite and in ({minimum_exclusive}, {maximum}]"
            ));
        }
        Ok(value)
    }

    pub(super) fn read_nonempty_path(&self, name: &str, default: &str) -> Result<PathBuf, String> {
        match self.string(name)? {
            Some(value) if !value.trim().is_empty() => Ok(PathBuf::from(value)),
            Some(_) => Err(format!("{name} must not be empty")),
            None => Ok(PathBuf::from(default)),
        }
    }

    pub(super) fn read_optional_string(&self, name: &str) -> Result<Option<String>, String> {
        self.string(name)
    }

    pub(super) fn read_string_or_default(&self, name: &str, default: String) -> String {
        self.lookup(name).ok().flatten().unwrap_or(default)
    }

    pub(super) fn read_optional_path(&self, name: &str) -> Option<PathBuf> {
        self.source
            .var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    pub(super) fn is_present(&self, name: &str) -> bool {
        self.source.var_os(name).is_some()
    }

    pub(super) fn has_unicode_value(&self, name: &str) -> bool {
        matches!(self.lookup(name), Ok(Some(_)))
    }

    fn string(&self, name: &str) -> Result<Option<String>, String> {
        self.lookup(name).map_err(|error| error.to_string())
    }
}

fn parse_positive_u64(name: &str, value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests;
