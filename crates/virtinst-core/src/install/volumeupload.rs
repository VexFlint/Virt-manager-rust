//! `install/volumeupload.py`: uploading kernel/initrd (or a generated
//! ISO) into a scratch pool through libvirt, for when the hypervisor
//! can't read our scratchdir.

use std::io::Read;

use virt::connect::Connect;
use virt::storage_vol::StorageVol;

use super::urlfetcher::basename;
use crate::diskbackend::{oserror_str, pool_of};
use crate::progress::Meter;
use crate::storage::{
    ensure_pool_is_running, find_free_name, find_free_pool_name, lookup_pool_by_path, NewPool,
    Pool, VolInstall,
};

/// `_build_pool`: the pool for `path`, defining a `boot-scratch` dir
/// pool there when there is none.
fn build_pool(conn: &Connect, path: &str) -> Result<Pool, String> {
    if let Some(pool) = lookup_pool_by_path(conn, path) {
        log::debug!("Existing pool '{}' found for {path}", pool.xml.name);
        ensure_pool_is_running(&pool.pool, true)?;
        return Ok(pool);
    }

    let name = find_free_pool_name(conn, "boot-scratch");
    log::debug!("Building storage pool: path={path} name={name}");
    let poolbuild = NewPool {
        name: name.clone(),
        pool_type: "dir".into(),
        target_path: Some(path.into()),
        ..NewPool::default()
    };
    // Explicitly don't build? since if we are creating this directory
    // we probably don't have correct perms
    poolbuild.install(conn, false)?;
    let pool =
        virt::storage_pool::StoragePool::lookup_by_name(conn, &name).map_err(|e| e.to_string())?;
    pool_of(&pool).ok_or_else(|| format!("Couldn't read storage pool '{name}'"))
}

/// `_MockStream`: the test suite's stand-in for a libvirt stream.
struct MockStream {
    data_size: Option<usize>,
}

impl MockStream {
    fn send(&mut self, data: &[u8]) -> usize {
        let size = *self.data_size.get_or_insert(data.len());
        let block_size = 128;
        let ret = size.min(block_size);
        self.data_size = Some(size.saturating_sub(block_size));
        ret
    }
}

enum Upload {
    Mock(MockStream),
    Real(virt::stream::Stream),
}

impl Upload {
    fn send(&mut self, data: &[u8]) -> Result<usize, String> {
        match self {
            Upload::Mock(m) => Ok(m.send(data)),
            Upload::Real(s) => s.send(data).map_err(|e| e.to_string()),
        }
    }

    fn finish(self) -> Result<(), String> {
        match self {
            Upload::Mock(_) => Ok(()),
            Upload::Real(s) => s.finish().map_err(|e| e.to_string()),
        }
    }
}

/// `_upload_file`: upload `src` into a new volume in `destpool`.
fn upload_file(
    conn: &Connect,
    meter: &mut dyn Meter,
    destpool: &Pool,
    src: &str,
    in_testsuite: bool,
) -> Result<StorageVol, String> {
    // Build stream object
    let mut stream = if in_testsuite {
        Upload::Mock(MockStream { data_size: None })
    } else {
        Upload::Real(virt::stream::Stream::new(conn, 0).map_err(|e| e.to_string())?)
    };

    // Build placeholder volume
    let size = std::fs::metadata(src)
        .map_err(|e| oserror_str(&e, Some(src)))?
        .len();
    let name = find_free_name(destpool, basename(src), "", &[])?;
    log::debug!("Generated volume name {name}");

    let vol_install = VolInstall::new(
        &name,
        Some(&destpool.xml),
        size as f64 / 1024.0 / 1024.0 / 1024.0,
        true,
        None,
    )?;
    vol_install.validate(conn)?;
    log::debug!(
        "Creating volume '{}' on pool '{}'",
        vol_install.name,
        destpool.xml.name
    );
    let vol = vol_install.install(conn)?;

    let res = (|| {
        // Register upload
        if let Upload::Real(s) = &stream {
            vol.upload(s, 0, size, 0).map_err(|e| e.to_string())?;
        }

        // Open source file
        let mut fileobj = std::fs::File::open(src).map_err(|e| oserror_str(&e, Some(src)))?;

        // Start transfer
        let mut total = 0u64;
        meter.start(&format!("Transferring '{}'", basename(src)), Some(size));
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = fileobj.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            // `safe_send`
            let mut data = &buf[..n];
            loop {
                let ret = stream.send(data)?;
                if ret == 0 || ret == data.len() {
                    break;
                }
                data = &data[ret..];
            }
            total += n as u64;
            meter.update(total);
        }
        Ok::<(), String>(())
    })();
    match res.and_then(|_| stream.finish()) {
        Ok(()) => {
            meter.end();
            Ok(vol)
        }
        Err(e) => {
            let _ = vol.delete(0);
            Err(e)
        }
    }
}

/// `upload_paths`: upload `pathlist` to the connection scratchdir pool,
/// returning the new paths and the volumes made.
pub fn upload_paths(
    conn: &Connect,
    system_scratchdir: &str,
    meter: &mut dyn Meter,
    pathlist: &[String],
    in_testsuite: bool,
) -> Result<(Vec<String>, Vec<StorageVol>), String> {
    // Build pool
    log::debug!("Uploading kernel/initrd media");
    let pool = build_pool(conn, system_scratchdir)?;

    let mut tmpvols: Vec<StorageVol> = vec![];
    let mut newpaths = vec![];
    for path in pathlist {
        match upload_file(conn, meter, &pool, path, in_testsuite) {
            Ok(vol) => {
                newpaths.push(vol.get_path().map_err(|e| e.to_string())?);
                tmpvols.push(vol);
            }
            Err(e) => {
                for vol in &tmpvols {
                    let _ = vol.delete(0);
                }
                return Err(e);
            }
        }
    }
    Ok((newpaths, tmpvols))
}
