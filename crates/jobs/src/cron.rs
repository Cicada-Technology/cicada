//! cron 表达式校验：与 `tokio-cron-scheduler` 的解析配置保持一致。
//!
//! 该调度器用 `croner` 且要求「秒必填 + 年月日/星期同真」；校验必须同口径，
//! 否则会出现「入库通过、调度器拒收」的错位。

/// 校验通过返回 `Ok(())`，否则返回人类可读的原因。
pub fn validate_cron(expr: &str) -> Result<(), String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("cron 表达式不能为空".to_string());
    }
    croner::parser::CronParser::builder()
        .seconds(croner::parser::Seconds::Required)
        .dom_and_dow(true)
        .build()
        .parse(expr)
        .map(|_| ())
        .map_err(|e| format!("cron 表达式非法：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_quartz_style_six_field() {
        assert!(validate_cron("0/10 * * * * *").is_ok());
        assert!(validate_cron("0 0 2 * * ?").is_ok());
    }

    #[test]
    fn rejects_garbage_and_empty() {
        assert!(validate_cron("").is_err());
        assert!(validate_cron("not a cron").is_err());
    }
}
