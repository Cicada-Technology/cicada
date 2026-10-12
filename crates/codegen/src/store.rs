//! `gen_table` / `gen_table_column` 读写。
//!
//! 一律带 `tenant_id` 与 `deleted_at IS NULL` 条件；列随表级联软删。

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

use crate::CodegenError;
use crate::entity::{
    GenTableActiveModel, GenTableCol, GenTableColumnActiveModel, GenTableColumnCol,
    GenTableColumnEntity, GenTableColumnModel, GenTableEntity, GenTableModel,
};
use crate::plan::{ColumnPlan, TablePlan, TplCategory, build_plan};
use crate::types;

/// 列表查询条件。
#[derive(Debug, Default, Clone)]
pub struct GenTableQuery {
    pub table_name: Option<String>,
    pub page: u64,
    pub page_size: u64,
}

/// 把一张表的生成计划写入元数据（存在则更新，并把列全量替换）。
pub async fn save_plan<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    plan: &TablePlan,
    operator: Option<&str>,
) -> Result<i64, CodegenError> {
    let now = Utc::now();
    let existing = GenTableEntity::find()
        .filter(GenTableCol::TenantId.eq(tenant))
        .filter(GenTableCol::TableName.eq(plan.table_name.as_str()))
        .filter(GenTableCol::DeletedAt.is_null())
        .one(db)
        .await
        .map_err(CodegenError::from_db)?;

    let table_id = match existing {
        Some(model) => {
            GenTableActiveModel {
                table_id: Set(model.table_id),
                table_comment: Set(plan.table_comment.clone()),
                class_name: Set(plan.class_name.clone()),
                module_name: Set(plan.module_name.clone()),
                business_name: Set(plan.business_name.clone()),
                function_name: Set(plan.function_name.clone()),
                function_author: Set(plan.function_author.clone()),
                tpl_category: Set(plan.tpl_category.as_str().to_string()),
                form_col_num: Set(plan.form_col_num),
                gen_type: Set(plan.gen_type.clone()),
                update_by: Set(operator.map(str::to_string)),
                update_time: Set(Some(now)),
                ..Default::default()
            }
            .update(db)
            .await
            .map_err(CodegenError::from_db)?;
            GenTableColumnEntity::delete_many()
                .filter(GenTableColumnCol::TenantId.eq(tenant))
                .filter(GenTableColumnCol::TableId.eq(model.table_id))
                .exec(db)
                .await
                .map_err(CodegenError::from_db)?;
            model.table_id
        }
        None => {
            GenTableActiveModel {
                tenant_id: Set(tenant),
                table_name: Set(plan.table_name.clone()),
                table_comment: Set(plan.table_comment.clone()),
                class_name: Set(plan.class_name.clone()),
                module_name: Set(plan.module_name.clone()),
                business_name: Set(plan.business_name.clone()),
                function_name: Set(plan.function_name.clone()),
                function_author: Set(plan.function_author.clone()),
                tpl_category: Set(plan.tpl_category.as_str().to_string()),
                form_col_num: Set(plan.form_col_num),
                gen_type: Set(plan.gen_type.clone()),
                create_by: Set(operator.map(str::to_string)),
                create_time: Set(now),
                ..Default::default()
            }
            .insert(db)
            .await
            .map_err(CodegenError::from_db)?
            .table_id
        }
    };

    for column in &plan.columns {
        GenTableColumnActiveModel {
            tenant_id: Set(tenant),
            table_id: Set(table_id),
            column_name: Set(column.column_name.clone()),
            column_comment: Set(column.column_comment.clone()),
            column_type: Set(column.column_type.clone()),
            rust_type: Set(column.rust_type.clone()),
            rust_field: Set(column.rust_field.clone()),
            is_pk: Set(column.is_pk),
            is_increment: Set(column.is_increment),
            is_required: Set(column.is_required),
            is_insert: Set(column.is_insert),
            is_edit: Set(column.is_edit),
            is_list: Set(column.is_list),
            is_query: Set(column.is_query),
            query_type: Set(column.query_type.clone()),
            html_type: Set(column.html_type.clone()),
            dict_type: Set(column.dict_type.clone()),
            sort: Set(column.sort),
            create_time: Set(now),
            ..Default::default()
        }
        .insert(db)
        .await
        .map_err(CodegenError::from_db)?;
    }

    Ok(table_id)
}

/// 分页列出已导入的表。
pub async fn list_tables<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    query: &GenTableQuery,
) -> Result<(Vec<GenTableModel>, u64), CodegenError> {
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 200);
    let mut select = GenTableEntity::find()
        .filter(GenTableCol::TenantId.eq(tenant))
        .filter(GenTableCol::DeletedAt.is_null());
    if let Some(name) = query.table_name.as_deref().filter(|s| !s.is_empty()) {
        select = select.filter(GenTableCol::TableName.contains(name));
    }
    let paginator = select
        .order_by_desc(GenTableCol::TableId)
        .paginate(db, page_size);
    let total = paginator.num_items().await.map_err(CodegenError::from_db)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(CodegenError::from_db)?;
    Ok((items, total))
}

/// 取一张表（含已删则视为不存在）。
pub async fn get_table<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    table_id: i64,
) -> Result<Option<GenTableModel>, CodegenError> {
    GenTableEntity::find()
        .filter(GenTableCol::TenantId.eq(tenant))
        .filter(GenTableCol::TableId.eq(table_id))
        .filter(GenTableCol::DeletedAt.is_null())
        .one(db)
        .await
        .map_err(CodegenError::from_db)
}

/// 取一张表的列（按 sort）。
pub async fn list_columns<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    table_id: i64,
) -> Result<Vec<GenTableColumnModel>, CodegenError> {
    GenTableColumnEntity::find()
        .filter(GenTableColumnCol::TenantId.eq(tenant))
        .filter(GenTableColumnCol::TableId.eq(table_id))
        .order_by_asc(GenTableColumnCol::Sort)
        .all(db)
        .await
        .map_err(CodegenError::from_db)
}

/// 软删一张表及其列。
pub async fn delete_table<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    table_id: i64,
    operator: Option<&str>,
) -> Result<bool, CodegenError> {
    let Some(model) = get_table(db, tenant, table_id).await? else {
        return Ok(false);
    };
    let now = Utc::now();
    GenTableActiveModel {
        table_id: Set(model.table_id),
        deleted_at: Set(Some(now)),
        update_by: Set(operator.map(str::to_string)),
        update_time: Set(Some(now)),
        ..Default::default()
    }
    .update(db)
    .await
    .map_err(CodegenError::from_db)?;
    GenTableColumnEntity::delete_many()
        .filter(GenTableColumnCol::TenantId.eq(tenant))
        .filter(GenTableColumnCol::TableId.eq(table_id))
        .exec(db)
        .await
        .map_err(CodegenError::from_db)?;
    Ok(true)
}

/// 从已存的元数据重建生成计划（预览/下载不再回读库表结构）。
pub fn plan_from_models(
    table: &GenTableModel,
    columns: &[GenTableColumnModel],
) -> Result<TablePlan, CodegenError> {
    if columns.is_empty() {
        return Err(CodegenError::State("该表没有列元数据".to_string()));
    }
    let mut plans = Vec::with_capacity(columns.len());
    for c in columns {
        let rust_type_base = c
            .rust_type
            .strip_prefix("Option<")
            .and_then(|s| s.strip_suffix('>'))
            .unwrap_or(c.rust_type.as_str())
            .to_string();
        plans.push(ColumnPlan {
            column_name: c.column_name.clone(),
            column_comment: c.column_comment.clone(),
            column_type: c.column_type.clone(),
            rust_type: c.rust_type.clone(),
            rust_type_base,
            rust_field: c.rust_field.clone(),
            pascal: crate::naming::pascal_case(&c.rust_field),
            is_pk: c.is_pk,
            is_increment: c.is_increment,
            is_required: c.is_required,
            is_insert: c.is_insert,
            is_edit: c.is_edit,
            is_list: c.is_list,
            is_query: c.is_query,
            query_type: c.query_type.clone(),
            html_type: c.html_type.clone(),
            dict_type: c.dict_type.clone(),
            sort: c.sort,
            framework: types::is_framework_column(&c.column_name),
        });
    }
    let tpl_category = TplCategory::parse(&table.tpl_category)?;
    // 与导入路径共用同一套树表推导：改完再预览也能得到一致的树元数据。
    let tree = crate::plan::apply_tree_contract(&mut plans, tpl_category)?;
    Ok(TablePlan {
        table_name: table.table_name.clone(),
        table_comment: table.table_comment.clone(),
        class_name: table.class_name.clone(),
        module_name: table.module_name.clone(),
        business_name: table.business_name.clone(),
        function_name: table.function_name.clone(),
        function_author: table.function_author.clone(),
        tpl_category,
        form_col_num: table.form_col_num,
        gen_type: table.gen_type.clone(),
        tree,
        columns: plans,
    })
}

/// 发现 + 建计划的便捷入口。
pub async fn plan_from_database(
    db: &cicada_infra::Db,
    cfg: &cicada_infra::GenConfig,
    table: &str,
    tpl_category: TplCategory,
) -> Result<TablePlan, CodegenError> {
    let discovered = crate::discovery::describe_table(db, table).await?;
    build_plan(&discovered, cfg, tpl_category)
}
