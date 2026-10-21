//! Local job data reads are bounded (security pass 2026-10-21, finding V-2): a data or
//! result URI from the chain naming a device, a directory or a file of the wrong size is
//! refused from its metadata, before any byte is read.

use pearl_job::data;

#[cfg(unix)]
#[test]
fn device_files_are_refused_without_reading() {
    // /dev/zero never ends: a plain fs::read would grow until the process is killed.
    let e = data::read_i32(std::path::Path::new("/dev/zero"), 16).unwrap_err().to_string();
    assert!(e.contains("not a regular file"), "{e}");
    let e = data::read_i8(std::path::Path::new("/dev/zero"), 16).unwrap_err().to_string();
    assert!(e.contains("not a regular file"), "{e}");
    let e = data::load("/dev/zero").err().unwrap().to_string();
    assert!(e.contains("not a regular file"), "{e}");
    let e = data::load("file:///dev/urandom").err().unwrap().to_string();
    assert!(e.contains("not a regular file"), "{e}");
}

#[test]
fn sizes_are_checked_before_reading() {
    let dir = std::env::temp_dir().join(format!("pearl-job-data-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("c.i32");
    std::fs::write(&f, vec![0u8; 64]).unwrap();
    assert_eq!(data::read_i32(&f, 16).unwrap().len(), 16);
    assert!(data::read_i32(&f, 15).unwrap_err().to_string().contains("expected 60"));
    // a manifest above the cap is not parsed
    let big = dir.join("manifest.json");
    std::fs::write(&big, vec![b' '; data::MAX_MANIFEST_BYTES as usize + 1]).unwrap();
    let e = data::load(big.to_str().unwrap()).err().unwrap().to_string();
    assert!(e.contains("at most"), "{e}");
    // a manifest whose A is a device is refused too
    let (a, b) = data::random_matrices(2, 2, 4, Some(1));
    let mp = data::save(&dir.join("job"), 2, 2, 4, &a, &b).unwrap();
    assert_eq!(data::load(mp.to_str().unwrap()).unwrap().a, a);
    #[cfg(unix)]
    {
        std::fs::write(&mp, r#"{"m":2,"n":2,"k":4,"a":"/dev/zero","b":"B.i8"}"#).unwrap();
        let e = data::load(mp.to_str().unwrap()).err().unwrap().to_string();
        assert!(e.contains("not a regular file"), "{e}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
