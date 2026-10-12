//! OpenAPI 快照（OpenAPI 快照进 CI，接口变更需在 PR 中显式确认）。
//!
//! 生成不需要运行中的服务（`ApiDoc` 只是类型上的 `OpenApi`），所以这就是一条普通测试，
//! 本地与 CI 同一条命令：`cargo test -p cicada-web --test openapi_snapshot`。
//! 故意改了接口形状时，用 `CICADA_OPENAPI_BLESS=1` 重写快照，
//! 然后**把 diff 留在 PR 里**——门禁要的不是「能过」，而是「改了什么被看见」。

use std::path::Path;

use utoipa::OpenApi;

use cicada_web::openapi::ApiDoc;

/// 快照落点：`docs/openapi.json`（crate 布局的 docs 目录含 OpenAPI 产物）。
const SNAPSHOT: &str = "../../docs/openapi.json";

fn rendered() -> String {
    let doc = ApiDoc::openapi();
    // 过一遍 `serde_json::Value`：Map 是 BTreeMap，键序因此稳定；数组顺序保持原样。
    let value = serde_json::to_value(&doc).expect("OpenAPI 文档必然可序列化");
    let normalized = serde_json::to_value(reindex(&value)).expect("规整后的文档必然可序列化");
    let mut text = serde_json::to_string_pretty(&normalized).expect("美化输出必然成功");
    text.push('\n');
    text
}

/// 递归排序对象键。`serde_json::Value` 的 Map 默认已按插入序或字典序，
/// 这里显式重建一次，避免依赖 feature 组合（`preserve_order` 会改变行为）。
fn reindex(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> =
                map.iter().map(|(k, v)| (k.clone(), reindex(v))).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(entries.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(reindex).collect())
        }
        other => other.clone(),
    }
}

#[test]
fn openapi_snapshot_matches_generated_document() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(SNAPSHOT);
    let text = rendered();

    if std::env::var("CICADA_OPENAPI_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &text).expect("写入快照");
        println!("已重写 {}", path.display());
        return;
    }

    let committed = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => panic!(
            "缺少 OpenAPI 快照 {}：先跑 CICADA_OPENAPI_BLESS=1 cargo test -p cicada-web --test openapi_snapshot",
            path.display()
        ),
    };
    if committed != text {
        // 只报差异规模与首处不同，完整 diff 交给 `git diff`（重写快照后自然显形）。
        let first = committed
            .chars()
            .zip(text.chars())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        let line = committed[..committed.len().min(first)]
            .matches('\n')
            .count()
            + 1;
        panic!(
            "OpenAPI 与已提交快照不一致（首处差异在第 {line} 行；\
             快照 {} 字节 / 生成 {} 字节）。确认这是有意的接口变更后：\
             CICADA_OPENAPI_BLESS=1 cargo test -p cicada-web --test openapi_snapshot",
            committed.len(),
            text.len()
        );
    }
}

#[test]
fn snapshot_covers_the_documented_surface() {
    // 光「和快照一致」还不够：操作数掉下去说明有接口被整段删掉了，
    // 而那正是横切能力要求「在 PR 里显式确认」的东西。阈值取当前实测值下方的余量，
    // 只用于拦「批量消失」，不拦单个新增（新增由上面的快照比对拦）。
    let doc = ApiDoc::openapi();
    let operations: usize = doc
        .paths
        .paths
        .values()
        .map(|item| {
            usize::from(item.get.is_some())
                + usize::from(item.post.is_some())
                + usize::from(item.put.is_some())
                + usize::from(item.delete.is_some())
                + usize::from(item.patch.is_some())
        })
        .sum();
    assert!(
        operations >= 150,
        "OpenAPI 只描述了 {operations} 个操作（当前基线 158），检查是不是有模块路由没接上"
    );
    let schemas = doc.components.map(|c| c.schemas.len()).unwrap_or(0);
    assert!(
        schemas >= 120,
        "注册的 schema 只有 {schemas} 个（当前基线 129）"
    );
}

/// 每条 `#[utoipa::path]` 声明的接口都必须被登记进 `ApiDoc::paths`（横切能力）。
///
/// 为什么扫源码：接口路径在项目里有**两处**声明——handler 上的 `#[utoipa::path]`
/// 和 `ApiDoc` 的 `paths(...)` 清单。少写第二处没有任何编译期信号，快照也照样自洽
/// （它比对的是「生成物 vs 已提交文件」，两边都缺同一条就一致）。实测漏过 13 条：
/// 接口清单那批接口 + `/api/v1/monitor/online-sessions`。
/// 走「装配 Router 再枚举」要 `AppState`（连库、只能 `--ignored`），而这条只需要源码。
#[test]
fn every_declared_path_is_registered_in_the_document() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let declared = declared_paths(&src);
    assert!(
        declared.len() >= 100,
        "只从源码扫到 {} 条 `#[utoipa::path]`，扫描逻辑大概失效了（基线 105）",
        declared.len()
    );

    let doc = ApiDoc::openapi();
    for (file, path) in &declared {
        assert!(
            doc.paths.paths.contains_key(path.as_str()),
            "{path}（{file}）有 `#[utoipa::path]` 却没登记进 `ApiDoc::paths`，\
             快照里看不见这条接口"
        );
    }
}

/// 递归收集 `src` 下所有 `#[utoipa::path(...)]` 块里的 `path = "…"`。
fn declared_paths(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(declared_paths(&path));
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("可读");
        for block in text.split("#[utoipa::path(").skip(1) {
            // 属性块到第一个 `)]` 结束；块外的 `path =` 不是接口声明，不能算进来。
            let Some(block) = block.split(")]").next() else {
                continue;
            };
            let Some(rest) = block.split("path = \"").nth(1) else {
                continue;
            };
            let Some(p) = rest.split('"').next() else {
                continue;
            };
            out.push((
                path.strip_prefix(dir)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                p.to_string(),
            ));
        }
    }
    out
}
