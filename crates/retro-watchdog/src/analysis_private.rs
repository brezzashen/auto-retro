//! Watchdog analysis directories are private evidence (session contract: dirs 0700, files 0600).
//! The CLI applies that itself: `retro-recall` copies the incident into `analysis/<id>/raw/`, and
//! `monitor-ack` seals the tree before validating it. Retro sessions had done both by hand
//! (`mkdir raw && chmod 700`, `cp incident`, `chmod 600 report.json`): 114 calls in 41 sessions in
//! conversation retro 83785989888175541344.
use anyhow::Result;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Sets every directory under `dir` (itself included) to 0700 and every file to 0600; symlinks are
/// left alone. Returns how many paths changed.
pub(crate) fn seal(dir: &Path) -> Result<usize> {
    let mut changed = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let meta = std::fs::symlink_metadata(&path)?;
        let want = if meta.is_dir() { 0o700 } else if meta.is_file() { 0o600 } else { continue };
        if meta.permissions().mode() & 0o777 != want {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(want))?;
            changed += 1;
        }
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
        }
    }
    Ok(changed)
}

/// Copies `incidents/<id>.json` to `analysis/<id>/raw/<id>.json` (0600) unless a copy exists.
pub(crate) fn seed_raw(state: &Path, id: &str) -> Result<()> {
    let raw = state.join("analysis").join(id).join("raw");
    let copy = raw.join(format!("{id}.json"));
    if copy.exists() {
        return Ok(());
    }
    std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700).recursive(true).create(&raw)?;
    std::fs::copy(state.join("incidents").join(format!("{id}.json")), &copy)?;
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cpu-watchdog-private-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn seal_makes_dirs_0700_and_files_0600_and_counts_changes() {
        let root = scratch("seal");
        let dir = root.join("analysis/1");
        std::fs::create_dir_all(dir.join("raw/resources")).unwrap();
        std::fs::write(dir.join("report.md"), "x").unwrap();
        std::fs::write(dir.join("raw/resources/a.json"), "{}").unwrap();
        for path in [dir.clone(), dir.join("raw"), dir.join("raw/resources")] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::set_permissions(dir.join("report.md"), std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(dir.join("raw/resources/a.json"), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(seal(&dir).unwrap(), 4);
        assert_eq!((mode(&dir), mode(&dir.join("raw/resources")), mode(&dir.join("report.md"))), (0o700, 0o700, 0o600));
        assert_eq!(seal(&dir).unwrap(), 0, "already sealed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn seed_raw_copies_the_incident_once() {
        let root = scratch("seed");
        std::fs::create_dir_all(root.join("incidents")).unwrap();
        std::fs::write(root.join("incidents/7.json"), "{\"id\":\"7\"}").unwrap();
        seed_raw(&root, "7").unwrap();
        let copy = root.join("analysis/7/raw/7.json");
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "{\"id\":\"7\"}");
        assert_eq!((mode(&copy), mode(copy.parent().unwrap())), (0o600, 0o700));
        std::fs::write(&copy, "kept").unwrap();
        seed_raw(&root, "7").unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "kept");
        std::fs::remove_dir_all(root).unwrap();
    }
}
