# 迁移

## 约定

- 迁移一律用 Rust + `sea_query` 写（`migrations/src/`，见「使用」），**不写裸 SQL**；
  只有插入原始数据可以用 SQL 片段。
- 一张表只在**一个**迁移里 `create_table`，且一次建出**最终形态**：
  尚未发布的表要加列就直接加到建表那处，不追加「先建后改」的迁移；
  发布之后的演进按 expand/contract（加可空列 → 回填 → 切读 → 删旧列）。
- 已合并到主干的迁移文件不再修改，只追加新迁移；CI 校验空库 up → down 可往返。

## 使用

安装sea-orm-cli

```sh
cargo install sea-orm-cli
```

创建新的迁移文件

```sh
# 创建枚举
sea-orm-cli migrate generate create_enum
# 建表
sea-orm-cli migrate generate create_***_table
# 函数
sea-orm-cli migrate generate create_or_replace_function
# 插入默认数据
sea-orm-cli migrate generate insert_***_data
```

迁移

```sh
sea-orm-cli migrate up
```

回滚

```sh
# 逐个回滚
sea-orm-cli migrate down
# 批量回滚
sea-orm-cli migrate down -n 10
```

生成model

```sh
sea-orm-cli generate entity -o src/model --with-serde both
```

生成带ToSchema的实体

```sh
sea-orm-cli generate entity \
-o src/model \
--with-serde both \
--model-extra-derives "ToSchema" \
--model-extra-attributes "schema(rename_all = \"lowercase\")" \
--enum-extra-derives "ToSchema" \
--enum-extra-attributes "schema(rename_all = \"lowercase\")"
```

一些不能ToSchema的字段按以下方式处理

```sh
#[schema(value_type = String, format = "date-time")]
```

## 数据边界

迁移只负责从空库建立 cicada 自己的 schema。已有数据由使用者在业务侧自行处理，框架不内置这一层。
