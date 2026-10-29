//! Upstream's `virtManager/lib/keyring.py` (`vmmKeyring`): graphical
//! console passwords kept in the freedesktop Secret Service, under the
//! same item label (`vm-console-<uuid>`) and attributes (`uuid`,
//! `hvuri`) as upstream, so a password saved by the Python virt-manager
//! is found here and the other way round. The console username lives in
//! the per-VM `console-username` setting, as upstream.
//!
//! Upstream makes blocking Gio D-Bus calls to `org.freedesktop.secrets`
//! from the GTK thread; here `oo7` makes the same calls as futures
//! awaited on the GTK main context, except reading a secret back, which
//! goes straight to `Item.GetSecret` (see [`VmmKeyring::secret_bytes`]
//! for why). Without a session bus or a Secret
//! Service the keyring is simply unavailable: the error is logged and
//! the console's "Save this password in your keyring" box stays
//! insensitive, as upstream.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use futures_channel::oneshot;
use oo7::dbus::{Item, Service};
use oo7::zbus::{self, zvariant};
use oo7::{ContentType, Secret};
use zeroize::Zeroizing;

use crate::domain::VmmDomain;

/// `_vmmSecret`. The password is wiped from memory when dropped and
/// never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct VmmSecret {
    pub name: String,
    pub secret: Zeroizing<String>,
    pub attributes: HashMap<String, String>,
}

impl std::fmt::Debug for VmmSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmmSecret")
            .field("name", &self.name)
            .field("secret", &"<redacted>")
            .field("attributes", &self.attributes)
            .finish()
    }
}

/// The attributes `_find_secret_item_path` searches by and
/// `set_console_password` stores: `{"uuid": uuid, "hvuri": hvuri}`.
pub fn secret_attributes(uuid: &str, hvuri: &str) -> HashMap<String, String> {
    HashMap::from([
        ("uuid".to_string(), uuid.to_string()),
        ("hvuri".to_string(), hvuri.to_string()),
    ])
}

/// `_get_secret_name`.
pub fn secret_name(uuid: &str) -> String {
    format!("vm-console-{uuid}")
}

/// `[ord(v) for v in secret.get_secret()]`: one byte per character.
/// Like upstream's `ay` variant, a character past U+00FF can't be
/// stored and fails the whole add.
pub fn encode_secret(secret: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    // Sized up front so no reallocation leaves a copy behind.
    let mut out = Zeroizing::new(Vec::with_capacity(secret.len()));
    for c in secret.chars() {
        let byte = u8::try_from(u32::from(c)).map_err(|_| "character out of range".to_string())?;
        out.push(byte);
    }
    Ok(out)
}

/// `"".join([chr(c) for c in secretbytes])`.
pub fn decode_secret(bytes: &[u8]) -> Zeroizing<String> {
    // At most two UTF-8 bytes per Latin-1 character: no reallocation.
    let mut out = Zeroizing::new(String::with_capacity(bytes.len() * 2));
    out.extend(bytes.iter().map(|&b| char::from(b)));
    out
}

/// `_get_secret`'s attribute copy: only `hvuri` and `uuid` are kept.
pub fn filter_attributes(attrs: HashMap<String, String>) -> HashMap<String, String> {
    attrs
        .into_iter()
        .filter(|(k, _)| k == "hvuri" || k == "uuid")
        .collect()
}

/// The VM's connection URI (`vm.conn.get_uri()`).
fn hvuri(vm: &VmmDomain) -> String {
    vm.conn().map(|c| c.get_uri()).unwrap_or_default()
}

const SECRETS_NAME: &str = "org.freedesktop.secrets";
const SECRETS_PATH: &str = "/org/freedesktop/secrets";

/// A `plain` Secret Service session on a connection of its own.
struct PlainSession {
    cnx: zbus::Connection,
    path: zvariant::OwnedObjectPath,
}

pub struct VmmKeyring {
    /// `self._collection is not None` upstream: an open Secret Service
    /// session.
    service: Option<Service>,
    /// For reading secrets; see [`Self::secret_bytes`].
    plain: RefCell<Option<Rc<PlainSession>>>,
}

enum Instance {
    /// The session is being opened; these callers wait for it.
    Pending(Vec<oneshot::Sender<Rc<VmmKeyring>>>),
    Ready(Rc<VmmKeyring>),
}

thread_local! {
    static INSTANCE: RefCell<Option<Instance>> = const { RefCell::new(None) };
}

impl VmmKeyring {
    /// `vmmKeyring.get_instance()`: the first call opens the Secret
    /// Service session, later ones share it.
    pub async fn get_instance() -> Rc<VmmKeyring> {
        let rx = INSTANCE.with(|i| {
            let mut i = i.borrow_mut();
            match &mut *i {
                Some(Instance::Ready(k)) => Ok(k.clone()),
                Some(Instance::Pending(waiters)) => {
                    let (tx, rx) = oneshot::channel();
                    waiters.push(tx);
                    Err(Some(rx))
                }
                None => {
                    *i = Some(Instance::Pending(Vec::new()));
                    Err(None)
                }
            }
        });
        match rx {
            Ok(k) => k,
            Err(Some(rx)) => match rx.await {
                Ok(k) => k,
                Err(_) => Rc::new(VmmKeyring {
                    service: None,
                    plain: RefCell::new(None),
                }),
            },
            Err(None) => {
                let keyring = Rc::new(VmmKeyring::new().await);
                let prev = INSTANCE.with(|i| i.replace(Some(Instance::Ready(keyring.clone()))));
                if let Some(Instance::Pending(waiters)) = prev {
                    for tx in waiters {
                        let _ = tx.send(keyring.clone());
                    }
                }
                keyring
            }
        }
    }

    async fn new() -> VmmKeyring {
        let service = match Service::new().await {
            Ok(service) => Some(service),
            Err(e) => {
                eprintln!("Error determining keyring: {e}");
                None
            }
        };
        VmmKeyring {
            service,
            plain: RefCell::new(None),
        }
    }

    /// `_find_secret_item_path`: Secret Service `SearchItems` over all
    /// collections; only an unlocked match counts.
    async fn find_secret_item(
        &self,
        uuid: &str,
        hvuri: &str,
    ) -> Result<Option<Item>, oo7::dbus::Error> {
        let Some(service) = &self.service else {
            return Ok(None);
        };
        let attributes = secret_attributes(uuid, hvuri);
        let mut locked = false;
        for collection in service.collections().await? {
            for item in collection.search_items(&attributes).await? {
                if item.is_locked().await? {
                    locked = true;
                } else {
                    return Ok(Some(item));
                }
            }
        }
        if locked {
            eprintln!("Item found, but it's locked");
        }
        Ok(None)
    }

    /// `_add_secret`: `CreateItem` with `replace=True` in the collection
    /// behind the `default` alias.
    async fn add_secret(&self, secret: &VmmSecret) {
        let Some(service) = &self.service else {
            return;
        };
        let res: Result<(), String> = async {
            let bytes = encode_secret(&secret.secret)?;
            let collection = service
                .with_alias(Service::DEFAULT_COLLECTION)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("No default keyring collection")?;
            collection
                .create_item(
                    &secret.name,
                    &secret.attributes,
                    Secret::with_content_type(ContentType::Text, bytes.as_slice()),
                    true,
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        .await;
        if let Err(e) = res {
            eprintln!("Failed to add keyring secret: {e}");
        }
    }

    /// `_del_secret`.
    async fn del_secret(&self, uuid: &str, hvuri: &str) {
        let res = async {
            if let Some(item) = self.find_secret_item(uuid, hvuri).await? {
                item.delete(None).await?;
            }
            Ok::<(), oo7::dbus::Error>(())
        }
        .await;
        if let Err(e) = res {
            eprintln!("Failed to delete keyring secret: {e}");
        }
    }

    /// Our own `plain` session, opened on first use, as upstream's.
    async fn plain_session(&self) -> Result<Rc<PlainSession>, zbus::Error> {
        if let Some(session) = self.plain.borrow().clone() {
            return Ok(session);
        }
        let cnx = zbus::Connection::session().await?;
        let reply = cnx
            .call_method(
                Some(SECRETS_NAME),
                SECRETS_PATH,
                Some("org.freedesktop.Secret.Service"),
                "OpenSession",
                &("plain", zvariant::Value::from("")),
            )
            .await?;
        let (_, path): (zvariant::OwnedValue, zvariant::OwnedObjectPath) =
            reply.body().deserialize()?;
        let session = Rc::new(PlainSession { cnx, path });
        self.plain.replace(Some(session.clone()));
        Ok(session)
    }

    /// `Item.GetSecret(session)[2]`, made directly as upstream does:
    /// oo7's `Item::secret` refuses any content type but `text/plain`
    /// and `application/octet-stream`, and upstream stores its
    /// passwords as `text/plain; charset=utf8`.
    async fn secret_bytes(&self, item: &Item) -> Result<Zeroizing<Vec<u8>>, zbus::Error> {
        let session = self.plain_session().await?;
        let reply = session
            .cnx
            .call_method(
                Some(SECRETS_NAME),
                item.path().clone(),
                Some("org.freedesktop.Secret.Item"),
                "GetSecret",
                &(&session.path,),
            )
            .await?;
        let ((_, _, value, _),): ((zvariant::OwnedObjectPath, Vec<u8>, Vec<u8>, String),) =
            reply.body().deserialize()?;
        Ok(Zeroizing::new(value))
    }

    /// `_get_secret`.
    async fn get_secret(&self, uuid: &str, hvuri: &str) -> Option<VmmSecret> {
        let res = async {
            let item = self
                .find_secret_item(uuid, hvuri)
                .await
                .map_err(|e| e.to_string())?;
            let Some(item) = item else {
                return Ok(None);
            };
            let secret = self.secret_bytes(&item).await.map_err(|e| e.to_string())?;
            let label = item.label().await.map_err(|e| e.to_string())?;
            let attributes = item.attributes().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(Some(VmmSecret {
                name: label,
                secret: decode_secret(&secret),
                attributes: filter_attributes(attributes),
            }))
        }
        .await;
        res.unwrap_or_else(|e| {
            eprintln!("Failed to get keyring secret uuid={uuid:?} hvuri={hvuri:?}: {e}");
            None
        })
    }

    // Public API

    pub fn is_available(&self) -> bool {
        self.service.is_some()
    }

    /// `get_console_password`: `(password, username)`, both empty when
    /// nothing is stored.
    pub async fn get_console_password(&self, vm: &VmmDomain) -> (Zeroizing<String>, String) {
        if !self.is_available() {
            return (Zeroizing::default(), String::new());
        }
        let Some(secret) = self.get_secret(&vm.get_uuid(), &hvuri(vm)).await else {
            return (Zeroizing::default(), String::new());
        };
        (secret.secret, vm.get_console_username())
    }

    /// `set_console_password`.
    pub async fn set_console_password(&self, vm: &VmmDomain, password: &str, username: &str) {
        if !self.is_available() {
            return;
        }
        let uuid = vm.get_uuid();
        let secret = VmmSecret {
            name: secret_name(&uuid),
            secret: Zeroizing::new(password.to_string()),
            attributes: secret_attributes(&uuid, &hvuri(vm)),
        };
        vm.set_console_username(username);
        self.add_secret(&secret).await;
    }

    /// `del_console_password`.
    pub async fn del_console_password(&self, vm: &VmmDomain) {
        if !self.is_available() {
            return;
        }
        self.del_secret(&vm.get_uuid(), &hvuri(vm)).await;
        vm.del_console_username();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_label_and_attributes_match_upstream() {
        let uuid = "12345678-1234-1234-1234-123456789012";
        assert_eq!(
            secret_name(uuid),
            "vm-console-12345678-1234-1234-1234-123456789012"
        );
        let attrs = secret_attributes(uuid, "qemu:///system");
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs["uuid"], uuid);
        assert_eq!(attrs["hvuri"], "qemu:///system");
    }

    #[test]
    fn secret_bytes_are_one_per_character() {
        assert_eq!(*encode_secret("pass").unwrap(), b"pass");
        // ord('é') == 0xe9: Latin-1, not UTF-8, same as upstream.
        assert_eq!(*encode_secret("é").unwrap(), vec![0xe9]);
        assert!(encode_secret("€").is_err());
        assert_eq!(*decode_secret(&[0x70, 0xe9]), "pé");
        let s = "s3cr\u{ff}t";
        assert_eq!(*decode_secret(&encode_secret(s).unwrap()), s);
    }

    #[test]
    fn secrets_never_print() {
        let secret = VmmSecret {
            name: secret_name("u"),
            secret: Zeroizing::new("hunter2".into()),
            attributes: secret_attributes("u", "h"),
        };
        let shown = format!("{secret:?}");
        assert!(shown.contains("<redacted>"));
        assert!(!shown.contains("hunter2"));
        let err = encode_secret("pass€word").unwrap_err();
        assert!(!err.contains('€'));
    }

    #[test]
    fn only_uuid_and_hvuri_attributes_are_kept() {
        let attrs = HashMap::from([
            ("uuid".to_string(), "u".to_string()),
            ("hvuri".to_string(), "h".to_string()),
            ("xdg:schema".to_string(), "x".to_string()),
        ]);
        let kept = filter_attributes(attrs);
        assert_eq!(kept, secret_attributes("u", "h"));
    }

    /// Against a live Secret Service: run with `--ignored` in a session
    /// that has one. `VMM_KEYRING_EXPECT=uuid|hvuri|password` also reads
    /// an item stored by upstream's keyring.py; the `rust-console` item
    /// is left behind for the reverse check.
    #[test]
    #[ignore]
    fn live_secret_service_roundtrip() {
        gtk::glib::MainContext::new().block_on(async {
            let k = VmmKeyring::get_instance().await;
            assert!(k.is_available(), "no Secret Service on the session bus");
            let hvuri = "test:///default";

            let mut secret = VmmSecret {
                name: secret_name("rust-console"),
                secret: Zeroizing::new("first".into()),
                attributes: secret_attributes("rust-console", hvuri),
            };
            k.add_secret(&secret).await;
            // replace=True: same attributes, new password.
            secret.secret = Zeroizing::new("s3cr\u{e9}t".into());
            k.add_secret(&secret).await;
            assert_eq!(k.get_secret("rust-console", hvuri).await, Some(secret));

            let tmp = VmmSecret {
                name: secret_name("rust-tmp"),
                secret: Zeroizing::new("gone".into()),
                attributes: secret_attributes("rust-tmp", hvuri),
            };
            k.add_secret(&tmp).await;
            assert!(k.get_secret("rust-tmp", hvuri).await.is_some());
            k.del_secret("rust-tmp", hvuri).await;
            assert_eq!(k.get_secret("rust-tmp", hvuri).await, None);

            if let Ok(expect) = std::env::var("VMM_KEYRING_EXPECT") {
                let parts: Vec<&str> = expect.splitn(3, '|').collect();
                let got = k.get_secret(parts[0], parts[1]).await;
                let got = got.expect("item stored by upstream's keyring.py");
                assert_eq!(*got.secret, parts[2]);
                assert_eq!(got.name, secret_name(parts[0]));
                assert_eq!(got.attributes, secret_attributes(parts[0], parts[1]));
            }
        });
    }
}
