//! `install/cloudinit.py`: the `--cloud-init` data and the files that go
//! on its `cidata` ISO.

use std::io::Write;

use super::urlfetcher::{direct_fetcher, named_tempfile};
use crate::progress::make_meter;

/// `CloudInitData`.
#[derive(Clone, Debug, Default)]
pub struct CloudInitData {
    pub disable: Option<bool>,
    pub root_password_generate: Option<bool>,
    pub root_password_file: Option<String>,
    generated_root_password: Option<String>,
    pub root_ssh_key: Option<String>,
    pub clouduser_ssh_key: Option<String>,
    pub user_data: Option<String>,
    pub meta_data: Option<String>,
    pub network_config: Option<String>,
}

/// `_get_password`: the first line of `pwdfile`.
pub(crate) fn get_password(pwdfile: &str) -> Result<String, String> {
    let content = std::fs::read_to_string(pwdfile)
        .map_err(|e| crate::diskbackend::oserror_str(&e, Some(pwdfile)))?;
    let line = content.split_inclusive('\n').next().unwrap_or("");
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

impl CloudInitData {
    fn generate_password(&mut self) -> String {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        self.generated_root_password
            .get_or_insert_with(|| {
                let bytes = uuid::Uuid::new_v4().into_bytes();
                let more = uuid::Uuid::new_v4().into_bytes();
                bytes
                    .iter()
                    .zip(more.iter())
                    .map(|(a, b)| {
                        let n = (*a as usize) << 8 | *b as usize;
                        CHARS[n % CHARS.len()] as char
                    })
                    .collect()
            })
            .clone()
    }

    /// `get_password_if_generated`.
    pub fn get_password_if_generated(&mut self) -> Option<String> {
        if self.root_password_generate == Some(true) {
            return Some(self.generate_password());
        }
        None
    }

    fn get_root_password(&mut self) -> Result<Option<String>, String> {
        if let Some(f) = &self.root_password_file {
            return get_password(f).map(Some);
        }
        Ok(self.get_password_if_generated())
    }

    fn create_userdata_content(&mut self) -> Result<String, String> {
        let mut content = "#cloud-config\n".to_string();

        let generate = self.root_password_generate == Some(true);
        if generate || self.root_password_file.is_some() {
            let rootpass = self.get_root_password()?.unwrap_or_else(|| "None".into());
            content += "chpasswd:\n";
            content += "  list: |\n";
            content += &format!("    root:{rootpass}\n");
        }
        if generate {
            content += "  expire: True\n";
        } else if self.root_password_file.is_some() {
            content += "  expire: False\n";
        }

        if let Some(f) = &self.root_ssh_key {
            let rootkey = get_password(f)?;
            content += "users:\n";
            content += "  - default\n";
            content += "  - name: root\n";
            content += "    ssh_authorized_keys:\n";
            content += &format!("      - {rootkey}\n");
        }

        if let Some(f) = &self.clouduser_ssh_key {
            let userkey = get_password(f)?;
            content += "ssh_authorized_keys:\n";
            content += &format!("  - {userkey}\n");
        }

        if self.disable == Some(true) {
            content += "runcmd:\n";
            content += "- echo \"Disabled by virt-install\" > /etc/cloud/cloud-init.disabled\n";
        }

        let clean_content: String = content
            .split_inclusive('\n')
            .map(|l| match l.find("root:") {
                Some(i) => {
                    let nl = if l.ends_with('\n') { "\n" } else { "" };
                    format!("{}root:[SCRUBBLED]{nl}", &l[..i])
                }
                None => l.to_string(),
            })
            .collect();
        if std::env::var_os("VIRTINST_TEST_SUITE_PRINT_CLOUDINIT").is_some() {
            println!("{clean_content}");
        }
        log::debug!("Generated cloud-init userdata: \n{clean_content}");
        Ok(content)
    }

    /// `_CloudInitConfig.add_filepair`.
    fn add_filepair(
        filepairs: &mut Vec<(String, String)>,
        destfile: &str,
        config: Option<&str>,
        content: Option<String>,
        scratchdir: &str,
    ) -> Result<(), String> {
        let file = if let Some(config) = config {
            // `_fetch_file`
            log::debug!("Using '{destfile}' content from path='{config}'");
            let mut meter = make_meter(true);
            let mut fetcher = direct_fetcher(scratchdir);
            Some(fetcher.acquire_file(config, None, &mut meter)?)
        } else {
            // `_create_file`
            match content.filter(|c| !c.is_empty()) {
                None => None,
                Some(content) => {
                    let (mut f, path) =
                        named_tempfile(scratchdir, "virtinst-", &format!("-{destfile}"))?;
                    f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
                    Some(path)
                }
            }
        };
        if let Some(file) = file {
            filepairs.push((file, destfile.to_string()));
        }
        Ok(())
    }

    /// `create_files`: (local file, name on the ISO) pairs.
    pub fn create_files(&mut self, scratchdir: &str) -> Result<Vec<(String, String)>, String> {
        let mut filepairs = vec![];
        let res = (|| {
            Self::add_filepair(
                &mut filepairs,
                "meta-data",
                self.meta_data.clone().as_deref(),
                Some(String::new()),
                scratchdir,
            )?;
            let userdata = if self.user_data.is_none() {
                Some(self.create_userdata_content()?)
            } else {
                None
            };
            Self::add_filepair(
                &mut filepairs,
                "user-data",
                self.user_data.clone().as_deref(),
                userdata,
                scratchdir,
            )?;
            Self::add_filepair(
                &mut filepairs,
                "network-config",
                self.network_config.clone().as_deref(),
                None,
                scratchdir,
            )
        })();
        if let Err(e) = res {
            for (f, _) in &filepairs {
                let _ = std::fs::remove_file(f);
            }
            return Err(e);
        }
        Ok(filepairs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_password_is_stable_and_alnum() {
        let mut d = CloudInitData {
            root_password_generate: Some(true),
            ..CloudInitData::default()
        };
        let p = d.get_password_if_generated().unwrap();
        assert_eq!(p.len(), 16);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(d.get_password_if_generated().unwrap(), p);
    }

    #[test]
    fn userdata_disable_and_password() {
        let mut d = CloudInitData {
            root_password_generate: Some(true),
            disable: Some(true),
            ..CloudInitData::default()
        };
        let c = d.create_userdata_content().unwrap();
        assert!(c.starts_with("#cloud-config\nchpasswd:\n  list: |\n    root:"));
        assert!(c.ends_with(
            "  expire: True\nruncmd:\n- echo \"Disabled by virt-install\" > /etc/cloud/cloud-init.disabled\n"
        ));
    }
}
