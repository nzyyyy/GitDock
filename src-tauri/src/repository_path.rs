use std::path::{Component, Path};

pub(crate) fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        Err("Invalid repository-relative path".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_outside_repository() {
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("/tmp/secret").is_err());
        assert!(validate_relative_path("src/main.rs").is_ok());
    }
}
