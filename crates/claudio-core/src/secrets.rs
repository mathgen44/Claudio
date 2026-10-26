//! Stockage des secrets (jetons d'API, jeton Hugging Face) dans le
//! Gestionnaire d'identifiants Windows, chiffré par DPAPI pour l'utilisateur courant.

use anyhow::{Result, bail};

const PREFIX: &str = "Claudio/";

/// Noms des secrets connus.
pub const HF_TOKEN: &str = "hf_token";
pub const TELEGRAM_TOKEN: &str = "telegram_token";

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result};
    use windows::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME};
    use windows::Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW,
        CredFree, CredReadW, CredWriteW,
    };
    use windows::core::{HSTRING, PWSTR};

    pub fn set(target: &str, secret: &str) -> Result<()> {
        let mut target_w: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let mut user_w: Vec<u16> = "claudio".encode_utf16().chain(Some(0)).collect();
        let blob = secret.as_bytes();
        let cred = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target_w.as_mut_ptr()),
            Comment: PWSTR::null(),
            LastWritten: FILETIME::default(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: PWSTR::null(),
            UserName: PWSTR(user_w.as_mut_ptr()),
        };
        unsafe { CredWriteW(&cred, 0) }.context("écriture dans le Gestionnaire d'identifiants")
    }

    pub fn get(target: &str) -> Result<Option<String>> {
        let mut out: *mut CREDENTIALW = std::ptr::null_mut();
        match unsafe { CredReadW(&HSTRING::from(target), CRED_TYPE_GENERIC, None, &mut out) } {
            Ok(()) => {
                // SAFETY : CredReadW a réussi, `out` pointe vers une structure valide
                // que l'on libère avec CredFree après copie.
                let value = unsafe {
                    let c = &*out;
                    let bytes =
                        std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
                    let s = String::from_utf8_lossy(bytes).into_owned();
                    CredFree(out as *const _);
                    s
                };
                Ok(Some(value))
            }
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(None),
            Err(e) => Err(e).context("lecture du Gestionnaire d'identifiants"),
        }
    }

    pub fn delete(target: &str) -> Result<()> {
        match unsafe { CredDeleteW(&HSTRING::from(target), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(e) => Err(e).context("suppression dans le Gestionnaire d'identifiants"),
        }
    }
}

fn target(name: &str) -> Result<String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!("nom de secret invalide : {name:?}");
    }
    Ok(format!("{PREFIX}{name}"))
}

/// Enregistre (ou remplace) un secret.
pub fn set(name: &str, secret: &str) -> Result<()> {
    imp::set(&target(name)?, secret)
}

/// Lit un secret ; `None` s'il n'existe pas.
pub fn get(name: &str) -> Result<Option<String>> {
    imp::get(&target(name)?)
}

/// Supprime un secret (sans erreur s'il n'existe pas).
pub fn delete(name: &str) -> Result<()> {
    imp::delete(&target(name)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let name = format!("test-{}", uuid::Uuid::new_v4().simple());
        assert_eq!(super::get(&name).unwrap(), None);
        super::set(&name, "s3cr3t-é").unwrap();
        assert_eq!(super::get(&name).unwrap().as_deref(), Some("s3cr3t-é"));
        super::delete(&name).unwrap();
        assert_eq!(super::get(&name).unwrap(), None);
    }

    #[test]
    fn rejects_bad_names() {
        assert!(super::set("../x", "y").is_err());
    }
}
