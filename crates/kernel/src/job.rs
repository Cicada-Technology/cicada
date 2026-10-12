//! 定时任务值对象：并发策略与触发来源。
//!
//! 放内核是为了让 `cicada-jobs`（领域）与 `cicada-ports`（端口）都能引用同一套
//! 纯值，而不产生 ports → 领域的反向依赖。

/// 并发策略（`sys_job.concurrency`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConcurrencyPolicy {
    /// 允许并发：每次触发都执行。
    Allow,
    /// 禁止并发：上一次未结束时跳过本次触发。
    Forbid,
    /// 队列：在实例内串行排队执行。
    Queue,
}

impl ConcurrencyPolicy {
    /// `sys_job.concurrency` 的数值编码。
    pub const fn code(self) -> i16 {
        match self {
            Self::Allow => 0,
            Self::Forbid => 1,
            Self::Queue => 2,
        }
    }

    /// 未知编码一律按最保守的「禁止并发」处理（失败关闭）。
    pub const fn from_code(code: i16) -> Self {
        match code {
            0 => Self::Allow,
            2 => Self::Queue,
            _ => Self::Forbid,
        }
    }
}

/// 触发来源（`sys_job_log.trigger_kind`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    /// 调度器触发。
    Scheduled,
    /// 人工触发（`PUT /monitor/jobs/{id}/run`）。
    Manual,
}

impl TriggerKind {
    pub const fn code(self) -> i16 {
        match self {
            Self::Scheduled => 0,
            Self::Manual => 1,
        }
    }

    /// 反解 `sys_job_log.trigger_kind`：未知编码按调度触发处理。
    pub const fn from_code(code: i16) -> Self {
        match code {
            1 => Self::Manual,
            _ => Self::Scheduled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_codes_round_trip_and_fail_closed() {
        assert_eq!(ConcurrencyPolicy::Allow.code(), 0);
        assert_eq!(ConcurrencyPolicy::Forbid.code(), 1);
        assert_eq!(ConcurrencyPolicy::Queue.code(), 2);
        assert_eq!(ConcurrencyPolicy::from_code(2), ConcurrencyPolicy::Queue);
        // 未知编码失败关闭为「禁止并发」。
        assert_eq!(ConcurrencyPolicy::from_code(9), ConcurrencyPolicy::Forbid);
    }

    #[test]
    fn trigger_kind_codes_round_trip() {
        assert_eq!(TriggerKind::Scheduled.code(), 0);
        assert_eq!(TriggerKind::Manual.code(), 1);
        assert_eq!(TriggerKind::from_code(1), TriggerKind::Manual);
        assert_eq!(TriggerKind::from_code(0), TriggerKind::Scheduled);
        assert_eq!(TriggerKind::from_code(7), TriggerKind::Scheduled);
    }
}
