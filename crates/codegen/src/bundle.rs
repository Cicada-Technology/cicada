//! 产物打包：把「路径 → 内容」写成一个 zip 字节流。

use std::io::{Cursor, Write};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::CodegenError;
use crate::render::Artifact;

/// 打包成一个 zip（deflate 压缩）。路径里的目录分隔符统一成 `/`。
pub fn zip_artifacts(artifacts: &[Artifact]) -> Result<Vec<u8>, CodegenError> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        for artifact in artifacts {
            let path = artifact.path.replace('\\', "/");
            writer
                .start_file(path, options)
                .map_err(|e| CodegenError::Bundle(e.to_string()))?;
            writer
                .write_all(artifact.content.as_bytes())
                .map_err(|e| CodegenError::Bundle(e.to_string()))?;
        }
        writer
            .finish()
            .map_err(|e| CodegenError::Bundle(e.to_string()))?;
    }
    Ok(buffer.into_inner())
}
