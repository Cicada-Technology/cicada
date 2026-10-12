fn main() {
    // rust-i18n 只监视它自己 crate 里的 locales 目录；消费端改 `locales/*.yml`
    // 不会让本 crate 重编，消息目录会静默停在旧版本（新增 key 一直回退成 fallback 语言）。
    println!("cargo:rerun-if-changed=locales");
}
