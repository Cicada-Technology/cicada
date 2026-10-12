//! 注册点（Slot）：注册契约的底层统一机制（FINAL.md ）。
//!
//! 框架与第三方、常见与非常见注册点都走同一条路：`Slot` 是**一等概念**，
//! 新增注册点只需声明一个新的 `static Slot<T>`，不改任何 trait。

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

/// 注册点的全局唯一键，例如 `"http.routes"` / `"crm.coupon_rules"`。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SlotKey(pub &'static str);

/// 同一槽收到多份贡献时的合并语义，由槽作者声明、由装配器执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// 全部收集，按模块装入序排列。
    Collect,
    /// 按稳定键去重；同键即冲突（fail fast）。
    Keyed,
    /// 至多一份贡献；第二份即冲突（端口绑定用）。
    Exclusive,
}

/// 一个一等注册点。声明它只写一行 `static`，不需要改任何 trait。
pub struct Slot<T: Send + Sync + 'static> {
    key: SlotKey,
    kind: &'static str,
    merge: Merge,
    key_of: Option<fn(&T) -> &'static str>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> Slot<T> {
    pub const fn collect(key: SlotKey, kind: &'static str) -> Self {
        Self {
            key,
            kind,
            merge: Merge::Collect,
            key_of: None,
            _t: PhantomData,
        }
    }

    pub const fn keyed(key: SlotKey, kind: &'static str, key_of: fn(&T) -> &'static str) -> Self {
        Self {
            key,
            kind,
            merge: Merge::Keyed,
            key_of: Some(key_of),
            _t: PhantomData,
        }
    }

    pub const fn exclusive(key: SlotKey, kind: &'static str) -> Self {
        Self {
            key,
            kind,
            merge: Merge::Exclusive,
            key_of: None,
            _t: PhantomData,
        }
    }

    pub const fn key(&self) -> SlotKey {
        self.key
    }

    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    pub const fn merge(&self) -> Merge {
        self.merge
    }
}

/// 一次类型擦除的贡献。模块经 `Manifest::extensions` 提交框架事先不知道的注册点。
pub struct Extension {
    pub(crate) slot: SlotKey,
    pub(crate) kind: &'static str,
    pub(crate) merge: Merge,
    pub(crate) key: Option<&'static str>,
    pub(crate) item: Arc<dyn Any + Send + Sync>,
}

impl Extension {
    /// 唯一的写入方法：把 `item` 贡献进 `slot`。
    pub fn contribute<T: Send + Sync + 'static>(slot: &'static Slot<T>, item: T) -> Self {
        let key = slot.key_of.map(|kf| kf(&item));
        Extension {
            slot: slot.key(),
            kind: slot.kind(),
            merge: slot.merge(),
            key,
            item: Arc::new(item),
        }
    }

    pub fn slot(&self) -> SlotKey {
        self.slot
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn merge(&self) -> Merge {
        self.merge
    }

    pub fn key(&self) -> Option<&'static str> {
        self.key
    }

    /// 供后续 phase 类型恢复后消费（`downcast_ref::<T>()`）。
    pub fn item(&self) -> &Arc<dyn Any + Send + Sync> {
        &self.item
    }
}
