//! API Key 安全存储（Windows 原生凭据管理器，keyring v3）。
//!
//! 服务名 `SpeakMirror` / 账户 `ai_api_key`。`settings.json` 不再落明文 Key：
//! - 保存设置时前端调 `set_api_key` 命令写系统凭据，store 里的 `apiKey` 清空；
//! - 启动/读取设置时 `settings::load` 从凭据管理器读回内存；
//! - 旧版明文 Key 首次读取时自动迁移进凭据管理器并清空 store 字段；
//! - 凭据管理器写入失败（如策略限制）→ 降级回明文存储，`set_api_key`
//!   返回 `secure: false` 与中文提示。
//!
//! 决策逻辑（迁移/降级）抽成纯函数 `resolve_key`，注入 `SecureStore`
//! 便于单测；keyring 本体在测试里真调一次存取删（测试专用账户，不碰生产条目）。

/// 生产条目：服务名 / 账户（与产品方案 §5.1 的本地隐私承诺一致：Key 不出本机）
pub const KEYRING_SERVICE: &str = "SpeakMirror";
pub const KEYRING_ACCOUNT: &str = "ai_api_key";

/// 系统凭据存储的最小抽象（生产 = keyring；测试 = mock / 真实 keyring 测试账户）
pub trait SecureStore {
    /// 读取；无条目或读失败（含空串）一律 None（读不到就走明文回退判定）
    fn get(&self) -> Option<String>;
    fn set(&self, key: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

/// keyring v3 真实实现（Windows 凭据管理器）
pub struct SystemKeyring {
    service: String,
    account: String,
}

impl SystemKeyring {
    /// 生产用的 AI API Key 条目
    pub fn ai_api_key() -> Self {
        Self { service: KEYRING_SERVICE.into(), account: KEYRING_ACCOUNT.into() }
    }
}

impl SecureStore for SystemKeyring {
    fn get(&self) -> Option<String> {
        let entry = keyring::Entry::new(&self.service, &self.account).ok()?;
        entry
            .get_password()
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }

    fn set(&self, key: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|e| format!("打开系统凭据管理器失败：{e}"))?;
        entry
            .set_password(key.trim())
            .map_err(|e| format!("写入系统凭据管理器失败：{e}"))
    }

    fn delete(&self) -> Result<(), String> {
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|e| format!("打开系统凭据管理器失败：{e}"))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            // 本就没有条目：视为已删除（幂等）
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("删除系统凭据失败：{e}")),
        }
    }
}

/// 生效 Key 的解析结果（迁移/降级决策，调用方据此改写 store）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedKey {
    /// 系统凭据里有 Key（凭据为准）；`clear_plain` = store 里还有遗留明文需要清掉
    Keyring { key: String, clear_plain: bool },
    /// store 里的旧明文已成功迁移进系统凭据（需清掉 store 明文）
    Migrated { key: String },
    /// 系统凭据不可用：保留明文降级使用（不清 store）
    Degraded { key: String },
    /// 两边都没有 Key
    None,
}

/// 纯决策函数：给定「凭据存储」与「store 里的明文」，决定生效 Key 与后续动作。
/// - 凭据里有 → 用凭据的（并把遗留明文标记待清理，凭据为准）
/// - 凭据里没有但明文非空 → 尝试迁移；成功 = Migrated（清明文），失败 = Degraded（保留）
/// - 两边都空 → None
pub fn resolve_key(store: &dyn SecureStore, plain: &str) -> ResolvedKey {
    if let Some(key) = store.get() {
        let clear_plain = !plain.trim().is_empty();
        return ResolvedKey::Keyring { key, clear_plain };
    }
    let plain = plain.trim();
    if plain.is_empty() {
        return ResolvedKey::None;
    }
    match store.set(plain) {
        Ok(()) => ResolvedKey::Migrated { key: plain.to_string() },
        Err(_) => ResolvedKey::Degraded { key: plain.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// mock 存储：可注入 set 失败，记录调用
    struct MockStore {
        value: RefCell<Option<String>>,
        fail_set: bool,
    }

    impl SecureStore for MockStore {
        fn get(&self) -> Option<String> {
            self.value.borrow().clone()
        }
        fn set(&self, key: &str) -> Result<(), String> {
            if self.fail_set {
                return Err("写入失败（模拟）".into());
            }
            *self.value.borrow_mut() = Some(key.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<(), String> {
            *self.value.borrow_mut() = None;
            Ok(())
        }
    }

    #[test]
    fn resolve_none_when_both_empty() {
        let mock = MockStore { value: RefCell::new(None), fail_set: false };
        assert_eq!(resolve_key(&mock, ""), ResolvedKey::None);
        assert_eq!(resolve_key(&mock, "   "), ResolvedKey::None);
    }

    #[test]
    fn resolve_keyring_wins_and_flags_stale_plaintext() {
        let mock = MockStore { value: RefCell::new(Some("sk-new".into())), fail_set: false };
        // 凭据里有、明文也有（旧值）→ 用凭据的，且要求清理明文
        assert_eq!(
            resolve_key(&mock, "sk-old"),
            ResolvedKey::Keyring { key: "sk-new".into(), clear_plain: true }
        );
        // 凭据里有、明文已清 → 无需清理
        assert_eq!(
            resolve_key(&mock, ""),
            ResolvedKey::Keyring { key: "sk-new".into(), clear_plain: false }
        );
    }

    #[test]
    fn resolve_migrates_plaintext_into_store() {
        let mock = MockStore { value: RefCell::new(None), fail_set: false };
        assert_eq!(
            resolve_key(&mock, " sk-legacy "),
            ResolvedKey::Migrated { key: "sk-legacy".into() }
        );
        assert_eq!(mock.get().as_deref(), Some("sk-legacy")); // 迁移确实写进了凭据
    }

    #[test]
    fn resolve_degrades_to_plaintext_when_store_fails() {
        let mock = MockStore { value: RefCell::new(None), fail_set: true };
        assert_eq!(
            resolve_key(&mock, "sk-legacy"),
            ResolvedKey::Degraded { key: "sk-legacy".into() }
        );
    }

    /// keyring 本体真实存取删一轮（测试专用服务/账户，不碰生产 ai_api_key 条目）。
    /// Windows 凭据管理器不可用的极端环境下会失败——本产品目标平台即 Windows。
    #[test]
    fn system_keyring_real_roundtrip_set_get_delete() {
        let store = SystemKeyring {
            service: "SpeakMirrorTest".into(),
            account: "ai_api_key_test".into(),
        };
        // 清场（上次测试残留的 NoEntry 视为成功）
        let _ = store.delete();
        assert_eq!(store.get(), None);

        store.set("sk-roundtrip-123").unwrap();
        assert_eq!(store.get().as_deref(), Some("sk-roundtrip-123"));

        // 覆盖写（更新 Key 的路径）
        store.set("sk-roundtrip-456").unwrap();
        assert_eq!(store.get().as_deref(), Some("sk-roundtrip-456"));

        store.delete().unwrap();
        assert_eq!(store.get(), None);
        // 再删一次幂等
        store.delete().unwrap();
    }

    #[test]
    fn production_constants_match_spec() {
        assert_eq!(KEYRING_SERVICE, "SpeakMirror");
        assert_eq!(KEYRING_ACCOUNT, "ai_api_key");
    }
}
