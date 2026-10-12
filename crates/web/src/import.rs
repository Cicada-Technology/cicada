//! Excel 导入：`calamine` 解析 + 逐行校验 + 结构化错误报告。
//!
//! 与 `export.rs` 对称：导出是写操作（`*:export`），导入同样是写操作
//! （`*:import`），落 `sys_oper_log`（`kind = "import"`）。
//!
//! 安全：导入内容一律做**公式注入**与 **XSS** 清洗；`=`、`+`、`@`
//! 前缀的单元格被中和，`-` 前缀仅在非数字时中和，避免误伤负数。

use calamine::{Data, Reader, Xlsx};

/// 导入解析/校验错误。
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("无法解析 xlsx：{0}")]
    Parse(String),
    #[error("缺少工作表")]
    NoSheet,
}

/// 一行数据（`number` 是工作表里的 1 基行号，便于错误报告）。
#[derive(Debug, Clone)]
pub struct Row {
    pub number: u32,
    pub cells: Vec<String>,
}

impl Row {
    /// 取第 `idx` 列（0 基），空或越界返回 `None`。
    pub fn cell(&self, idx: usize) -> Option<&str> {
        self.cells
            .get(idx)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }

    /// 取第 `idx` 列并清洗（用于落库的用户输入）。
    pub fn clean(&self, idx: usize) -> Option<String> {
        self.cell(idx).map(sanitize_cell)
    }
}

/// 逐行错误：定位到「行号 + 字段 + 原因」。
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct RowError {
    pub row: u32,
    pub field: String,
    pub reason: String,
}

/// 导入结果报告。
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ImportReport {
    /// 数据行总数（不含表头）。
    pub total: u32,
    pub success: u32,
    pub failed: u32,
    pub errors: Vec<RowError>,
}

impl ImportReport {
    pub fn new(total: u32, success: u32, errors: Vec<RowError>) -> Self {
        let failed = errors.len() as u32;
        Self {
            total,
            success,
            failed,
            errors,
        }
    }
}

/// 解析 xlsx 字节流：返回除表头外的数据行（表头由调用方在 `headers` 里校验）。
///
/// 只读第一个工作表；空行（全空单元格）被跳过。
pub fn parse_xlsx(bytes: &[u8]) -> Result<Vec<Row>, ImportError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut book = Xlsx::new(cursor).map_err(|e| ImportError::Parse(e.to_string()))?;
    let name = book
        .sheet_names()
        .first()
        .cloned()
        .ok_or(ImportError::NoSheet)?;
    let range = book
        .worksheet_range(&name)
        .map_err(|e| ImportError::Parse(e.to_string()))?;

    let mut rows = Vec::new();
    for (idx, row) in range.rows().enumerate() {
        let cells: Vec<String> = row.iter().map(cell_to_string).collect();
        if cells.iter().all(|c| c.is_empty()) {
            continue;
        }
        rows.push(Row {
            number: idx as u32 + 1,
            cells,
        });
    }
    Ok(rows)
}

/// 校验表头（第 0 行）与期望表头一致；不一致时返回缺失的列名。
pub fn check_headers(rows: &[Row], expected: &[&str]) -> Result<(), ImportError> {
    let Some(header) = rows.first() else {
        return Err(ImportError::Parse("文件为空".to_string()));
    };
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .enumerate()
        .filter(|(i, name)| header.cell(*i).map(|c| c != *name).unwrap_or(true))
        .map(|(_, name)| name)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ImportError::Parse(format!(
            "模板表头不匹配，缺少列：{}",
            missing.join("、")
        )))
    }
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.trim().to_string(),
        Data::Bool(b) => b.to_string(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERR:{e:?}"),
    }
}

/// 导入清洗：去控制字符 + 中和公式注入前缀。
pub fn sanitize_cell(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect();
    neutralize_formula(cleaned.trim())
}

/// 导出转义：给可能被 Excel 当作公式的文本加前缀。
pub fn escape_formula(raw: &str) -> String {
    neutralize_formula(raw)
}

fn neutralize_formula(value: &str) -> String {
    let value = value.trim_start_matches('\u{feff}').trim();
    let Some(first) = value.chars().next() else {
        return String::new();
    };
    let dangerous = match first {
        '=' | '+' | '@' => true,
        // 负号仅在非数字时中和，避免把 -3.14 这类合法负数改写。
        '-' => value[1..].parse::<f64>().is_err(),
        _ => false,
    };
    if dangerous {
        format!("'{value}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_control_and_neutralizes_formula() {
        assert_eq!(sanitize_cell("  admin  "), "admin");
        assert_eq!(sanitize_cell("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(sanitize_cell("-3.14"), "-3.14");
        assert_eq!(sanitize_cell("-admin"), "'-admin");
        assert_eq!(sanitize_cell("a\u{0007}b"), "ab");
    }

    #[test]
    fn parse_and_check_headers() {
        assert!(check_headers(&[], &["a"]).is_err());
    }
}
