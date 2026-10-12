//! 富文本入库前的白名单清洗（富文本内容存储前用白名单清洗）。
//!
//! 公告正文由后台管理员撰写，但渲染给全租户所有登录者看——能发公告不等于能在别人
//! 浏览器里执行脚本。白名单放在 `system`（`sys_notice` 的属主）而不是某个调用点，
//! 是为了让「入库的富文本一定先过这里」成为这个模块的规则，而不是某个 handler 的自觉。

use ammonia::Builder;

/// 允许的标签：够排版（段落、列表、标题、引用、代码、表格）与图文混排，
/// 其余标签整层拆掉只留文字——`script`/`iframe`/`form` 这类根本不在列。
const ALLOWED_TAGS: &[&str] = &[
    "p",
    "br",
    "div",
    "span",
    "strong",
    "b",
    "em",
    "i",
    "u",
    "s",
    "sub",
    "sup",
    "blockquote",
    "pre",
    "code",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "hr",
    "a",
    "img",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "td",
    "th",
];

/// 允许的属性。`style`、`class`、`on*` 一律不在列：样式与脚本都不是内容。
const ALLOWED_ATTRS: &[&str] = &[
    "href", "src", "alt", "title", "colspan", "rowspan", "start", "width", "height",
];

/// 链接与图片只认这三种协议。`javascript:` / `data:` 是富文本 XSS 的主通道，
/// 属性名合法不代表值合法，所以协议这一层单独拦。
const ALLOWED_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// 清洗一段 HTML；纯文本会原样保留（转义后）。
pub fn sanitize_html(html: &str) -> String {
    Builder::new()
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .generic_attributes(ALLOWED_ATTRS.iter().copied().collect())
        .url_schemes(ALLOWED_SCHEMES.iter().copied().collect())
        .clean(html)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_layout_tags_but_drops_scripts_and_iframes() {
        let cleaned = sanitize_html(
            "<p>今晚 <strong>23:00</strong> 停机</p><script>alert(document.cookie)</script><iframe src=\"https://evil\"></iframe>",
        );
        assert!(
            cleaned.contains("<p>") && cleaned.contains("<strong>23:00</strong>"),
            "{cleaned}"
        );
        assert!(!cleaned.contains("<script"), "{cleaned}");
        assert!(!cleaned.contains("<iframe"), "{cleaned}");
    }

    #[test]
    fn drops_event_handlers_and_inline_styles() {
        let cleaned =
            sanitize_html("<p onclick=\"steal()\" style=\"color:red\" class=\"x\">正文</p>");
        assert_eq!(cleaned, "<p>正文</p>", "属性只留白名单里的");
    }

    /// 属性名合法 ≠ 值合法：`href` 是允许的，但 `javascript:` 协议必须被去掉。
    /// 这条是本模块存在的理由，也是纯标签/属性白名单最容易被绕过的一处。
    #[test]
    fn rejects_javascript_and_data_urls() {
        let link = sanitize_html("<a href=\"javascript:alert(1)\">点我</a>");
        assert!(!link.contains("javascript:"), "{link}");
        assert!(
            !link.contains("href"),
            "协议不合法的链接不该留下 href：{link}"
        );
        let img = sanitize_html("<img src=\"data:text/html,<script>alert(1)</script>\">");
        assert!(!img.contains("data:"), "{img}");
    }

    #[test]
    fn keeps_plain_text_and_http_links() {
        let cleaned = sanitize_html("<a href=\"https://example.com/a?b=1\">公告</a> 详见邮件");
        assert!(
            cleaned.contains("href=\"https://example.com/a?b=1\""),
            "{cleaned}"
        );
        assert!(cleaned.contains("公告"), "{cleaned}");
        assert_eq!(sanitize_html("普通正文"), "普通正文");
    }
}
