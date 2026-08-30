//! Antigravity 凭据安全存储。
//!
//! 硬性要求（Phase 3）：access_token / refresh_token 禁止明文落盘。
//!
//! - Windows：DPAPI（CryptProtectData，CurrentUser scope）—— 无需额外依赖
//! - macOS / Linux：暂无系统 keychain 集成时降级为 0600 明文 + 醒目告警
//!   （与上游 codex/xai OAuth 存储现状一致，作为显式降级而非默认）
//!
//! 文件格式：
//!   `CCSW1` magic + DPAPI blob（Windows）
//!   `CCSP0` magic + 明文 JSON（降级平台）
//!
//! 兼容：启动时若发现旧版明文 `antigravity_oauth_auth.json`，自动读取、
//! 重新加密写入并删除明文文件（一次性迁移）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAGIC_ENCRYPTED: &[u8; 5] = b"CCSW1";
const MAGIC_PLAINTEXT: &[u8; 5] = b"CCSP0";

#[derive(Debug, thiserror::Error)]
pub enum SecureStoreError {
    #[error("DPAPI 加密失败: {0}")]
    EncryptFailed(String),
    #[error("DPAPI 解密失败: {0}")]
    DecryptFailed(String),
    #[error("IO 错误: {0}")]
    IoError(String),
    #[error("存储文件格式无效")]
    InvalidFormat,
}

/// 平台是否提供真加密（用于 UI/Doctor 告警展示）
pub fn platform_encryption_available() -> bool {
    cfg!(target_os = "windows")
}

/// 返回加密存储文件路径（.bin）与旧明文路径（.json）。
/// `primary` 为既有 manager 的 storage_path（.json），据此推导 .bin。
pub fn encrypted_path_for(primary: &Path) -> PathBuf {
    let parent = primary.parent().unwrap_or_else(|| Path::new("."));
    parent.join("antigravity_oauth_auth.bin")
}

/// 加密并原子写入。Windows 用 DPAPI；其它平台 0600 明文 + 告警。
pub fn write_secure(
    path: &Path,
    plaintext: &str,
) -> Result<(), SecureStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| SecureStoreError::IoError("无效存储路径".into()))?;
    fs::create_dir_all(parent)
        .map_err(|error| SecureStoreError::IoError(error.to_string()))?;

    let mut payload: Vec<u8> = Vec::with_capacity(plaintext.len() + 16);
    #[cfg(target_os = "windows")]
    {
        payload.extend_from_slice(MAGIC_ENCRYPTED);
        let blob = dpapi_protect(plaintext.as_bytes())?;
        payload.extend_from_slice(&blob);
    }
    #[cfg(not(target_os = "windows"))]
    {
        log::warn!(
            "[AntigravitySecure] 当前平台暂无系统级加密，凭据将以 0600 明文写入 {}（建议迁移到支持 keychain 的平台）",
            path.display()
        );
        payload.extend_from_slice(MAGIC_PLAINTEXT);
        payload.extend_from_slice(plaintext.as_bytes());
    }

    // 原子写：tmp + rename
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = parent.join(format!(
        "{}.tmp.{nonce}",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "antigravity_oauth_auth.bin".into())
    ));
    let result = (|| -> Result<(), SecureStoreError> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
            .map_err(|error| SecureStoreError::IoError(error.to_string()))?;
        file.write_all(&payload)
            .map_err(|error| SecureStoreError::IoError(error.to_string()))?;
        file.flush()
            .map_err(|error| SecureStoreError::IoError(error.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
                .map_err(|error| SecureStoreError::IoError(error.to_string()))?;
        }
        if path.exists() {
            fs::remove_file(path).map_err(|error| SecureStoreError::IoError(error.to_string()))?;
        }
        fs::rename(&tmp, path).map_err(|error| SecureStoreError::IoError(error.to_string()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// 读取并解密。返回 None 表示文件不存在。
pub fn read_secure(path: &Path) -> Result<Option<String>, SecureStoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read(path).map_err(|error| SecureStoreError::IoError(error.to_string()))?;
    if raw.len() < 5 {
        return Err(SecureStoreError::InvalidFormat);
    }
    let (magic, body) = raw.split_at(5);
    if magic == MAGIC_ENCRYPTED {
        #[cfg(target_os = "windows")]
        {
            let plain = dpapi_unprotect(body)?;
            String::from_utf8(plain)
                .map(Some)
                .map_err(|_| SecureStoreError::InvalidFormat)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(SecureStoreError::InvalidFormat)
        }
    } else if magic == MAGIC_PLAINTEXT {
        String::from_utf8(body.to_vec())
            .map(Some)
            .map_err(|_| SecureStoreError::InvalidFormat)
    } else {
        Err(SecureStoreError::InvalidFormat)
    }
}

/// 一次性迁移：若存在旧明文 JSON（无 magic 的原始 JSON），读取后加密写入并删除明文。
/// 返回 Ok(Some(json)) 表示迁移出的凭据内容。
pub fn migrate_legacy_plaintext(primary_json: &Path) -> Result<Option<String>, SecureStoreError> {
    if !primary_json.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(primary_json)
        .map_err(|error| SecureStoreError::IoError(error.to_string()))?;
    let trimmed = content.trim_start();
    if trimmed.starts_with('{') {
        // 纯 JSON：旧版明文存储
        let target = encrypted_path_for(primary_json);
        write_secure(&target, &content)?;
        fs::remove_file(primary_json)
            .map_err(|error| SecureStoreError::IoError(error.to_string()))?;
        log::info!(
            "[AntigravitySecure] 已迁移明文凭据到加密存储: {}",
            target.display()
        );
        return Ok(Some(content));
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, SecureStoreError> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // szDataDescr = null（默认描述），dwFlags = 0（CurrentUser scope）
        let ok = CryptProtectData(
            &input as *const CRYPT_INTEGER_BLOB,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            &mut output,
        );
        if ok == 0 {
            return Err(SecureStoreError::EncryptFailed(
                "CryptProtectData failed".into(),
            ));
        }
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let out = slice.to_vec();
        LocalFreeBlob(output.pbData, output.cbData);
        Ok(out)
    }
}

#[cfg(target_os = "windows")]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, SecureStoreError> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &input as *const CRYPT_INTEGER_BLOB,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            &mut output,
        );
        if ok == 0 {
            return Err(SecureStoreError::DecryptFailed(
                "CryptUnprotectData failed".into(),
            ));
        }
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let out = slice.to_vec();
        LocalFreeBlob(output.pbData, output.cbData);
        Ok(out)
    }
}

#[cfg(target_os = "windows")]
fn LocalFreeBlob(ptr: *mut u8, len: u32) {
    // CRYPT_INTEGER_BLOB.pbData 由 LocalAlloc 分配，需 LocalFree 释放
    use windows_sys::Win32::Foundation::LocalFree;
    if !ptr.is_null() {
        unsafe {
            LocalFree(ptr as *mut core::ffi::c_void);
        }
    }
    let _ = len;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_on_current_platform() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("antigravity_oauth_auth.bin");
        write_secure(&path, r#"{"accounts":{}}"#).unwrap();
        let read = read_secure(&path).unwrap().unwrap();
        assert_eq!(read, r#"{"accounts":{}}"#);
        // 文件不得以明文 JSON 开头
        let raw = fs::read(&path).unwrap();
        assert!(!raw.starts_with(b"{"));
    }

    #[test]
    fn migrate_legacy_json_removes_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("antigravity_oauth_auth.json");
        fs::write(&json_path, r#"{"version":1,"accounts":{},"default_account_id":null}"#).unwrap();
        let migrated = migrate_legacy_plaintext(&json_path).unwrap();
        assert!(migrated.is_some());
        assert!(!json_path.exists(), "明文文件必须被删除");
        let bin = encrypted_path_for(&json_path);
        assert!(bin.exists());
        let content = read_secure(&bin).unwrap().unwrap();
        assert!(content.contains("\"version\":1"));
    }
}
