//! Excel 导出：`rust_xlsxwriter` 生成 xlsx。
//!
//! 大表导出走「数据库分批读 + workbook 逐行写」：`constant_memory` 模式下每行
//! 写完即刷临时文件，内存不随行数增长（M2 出口条件：导出 10 万行内存稳定）。
//! 导出是写操作，一律带 `*:export` 权限点并写 `sys_oper_log`（`kind = "export"`）。

use std::future::Future;
use std::pin::Pin;

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use rust_xlsxwriter::{Format, Workbook, Worksheet, XlsxError};

/// 导出过程的错误：xlsx 生成失败或数据读取失败。
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("xlsx 生成失败：{0}")]
    Xlsx(#[from] XlsxError),
    #[error("导出数据读取失败：{0}")]
    Db(String),
}

impl From<sea_orm::DbErr> for ExportError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Db(e.to_string())
    }
}

/// 单次导出的行数上限：防止误导出全库拖垮数据库。
pub const MAX_EXPORT_ROWS: u64 = 100_000;

/// 分批读库的批大小。
pub const EXPORT_BATCH: u64 = 1_000;

const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// 写行回调返回的未来；借用 worksheet，逐行写入。
pub type WriteRows<'a> = Pin<Box<dyn Future<Output = Result<(), ExportError>> + Send + 'a>>;

/// 导出进度回调：每写完一批上报累计行数（异步导出用它更新 `sys_export_task.processed`）。
pub type Progress = Box<dyn FnMut(i64) + Send + 'static>;

/// 生成一个单 sheet 的 xlsx 字节流。
///
/// `write_rows` 拿到 worksheet 后按行写入（第 0 行是表头）；调用方内部自行
/// 分批读库，函数本身不感知数据来源。
pub async fn build<F>(
    sheet_name: &str,
    headers: &[&str],
    widths: &[f64],
    write_rows: F,
) -> Result<Vec<u8>, ExportError>
where
    F: for<'s> FnOnce(&'s mut Worksheet) -> WriteRows<'s>,
{
    let mut workbook = Workbook::new();
    {
        let sheet = workbook.add_worksheet_with_constant_memory();
        sheet.set_name(sheet_name)?;

        let header_fmt = Format::new().set_bold().set_background_color("#F2F2F2");
        for (col, title) in headers.iter().enumerate() {
            sheet.write_string_with_format(0, col as u16, *title, &header_fmt)?;
        }
        for (col, width) in widths.iter().enumerate() {
            sheet.set_column_width(col as u16, *width)?;
        }

        write_rows(sheet).await?;
    }
    workbook.save_to_buffer().map_err(ExportError::from)
}

/// 把 xlsx 字节流包成「附件下载」响应。
pub fn xlsx_response(filename: &str, bytes: Vec<u8>) -> Response {
    let ascii_fallback: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let disposition = format!(
        "attachment; filename=\"{ascii_fallback}\"; filename*=UTF-8''{}",
        percent_encode(filename)
    );
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static(XLSX_MIME)),
        (
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition)
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        ),
    ];
    (headers, bytes).into_response()
}

/// RFC 3986 百分号编码（仅用于 `filename*` 的 UTF-8 文件名）。
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_escapes_non_ascii() {
        assert_eq!(percent_encode("users.xlsx"), "users.xlsx");
        assert_eq!(percent_encode("用户.xlsx"), "%E7%94%A8%E6%88%B7.xlsx");
    }

    #[test]
    fn response_carries_attachment_headers() {
        let resp = xlsx_response("用户.xlsx", vec![1, 2, 3]);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), XLSX_MIME);
        let cd = resp
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cd.starts_with("attachment;"), "{cd}");
        assert!(
            cd.contains("filename*=UTF-8''%E7%94%A8%E6%88%B7.xlsx"),
            "{cd}"
        );
    }
}
