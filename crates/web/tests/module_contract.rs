use cicada_web::{
    AssembleError, Assembler, Extension, Manifest, Module, ModuleName, Permission, SeedKey, SeedOp,
    Slot, SlotKey, TableName,
};

struct TestModule {
    m: Manifest,
}

impl Module for TestModule {
    fn manifest(self: Box<Self>) -> Manifest {
        self.m
    }
}

fn module(name: &'static str) -> TestModule {
    TestModule {
        m: Manifest::new(name),
    }
}

#[test]
fn seal_succeeds_for_single_module() {
    let app = Assembler::new()
        .with(Box::new(module("cicada:system")))
        .seal()
        .unwrap();
    assert_eq!(app.len(), 1);
    assert_eq!(
        app.names().collect::<Vec<_>>(),
        vec![ModuleName("cicada:system")]
    );
}

#[test]
fn duplicate_module_is_rejected() {
    let err = Assembler::new()
        .with(Box::new(module("a")))
        .with(Box::new(module("a")))
        .seal()
        .unwrap_err();
    assert!(matches!(
        err.errors.as_slice(),
        [AssembleError::DuplicateModule(ModuleName("a"))]
    ));
}

#[test]
fn duplicate_permission_is_rejected() {
    let a = TestModule {
        m: Manifest::new("a").perms([Permission::new("system:user:add")]),
    };
    let b = TestModule {
        m: Manifest::new("b").perms([Permission::new("system:user:add")]),
    };
    let err = Assembler::new()
        .with(Box::new(a))
        .with(Box::new(b))
        .seal()
        .unwrap_err();
    assert!(matches!(
        err.errors.as_slice(),
        [AssembleError::DuplicatePermission { .. }]
    ));
}

#[test]
fn foreign_table_seed_is_rejected() {
    let m = TestModule {
        m: Manifest::new("a").seeds([SeedOp {
            key: SeedKey("k"),
            table: TableName("not_owned"),
        }]),
    };
    let err = Assembler::new().with(Box::new(m)).seal().unwrap_err();
    assert!(matches!(
        err.errors.as_slice(),
        [AssembleError::ForeignTableSeed { .. }]
    ));
}

#[test]
fn exclusive_slot_conflict_is_rejected() {
    static PORT: Slot<&'static str> = Slot::exclusive(SlotKey("port.x"), "端口 X");
    let a = TestModule {
        m: Manifest::new("a").extensions([Extension::contribute(&PORT, "a")]),
    };
    let b = TestModule {
        m: Manifest::new("b").extensions([Extension::contribute(&PORT, "b")]),
    };
    let err = Assembler::new()
        .with(Box::new(a))
        .with(Box::new(b))
        .seal()
        .unwrap_err();
    assert!(matches!(
        err.errors.as_slice(),
        [AssembleError::SlotConflict { .. }]
    ));
}

#[test]
fn keyed_slot_duplicate_is_rejected() {
    static RULES: Slot<(&'static str, i32)> = Slot::keyed(SlotKey("crm.rules"), "规则", |r| r.0);
    let a = TestModule {
        m: Manifest::new("a").extensions([Extension::contribute(&RULES, ("k", 1))]),
    };
    let b = TestModule {
        m: Manifest::new("b").extensions([Extension::contribute(&RULES, ("k", 2))]),
    };
    let err = Assembler::new()
        .with(Box::new(a))
        .with(Box::new(b))
        .seal()
        .unwrap_err();
    assert!(matches!(
        err.errors.as_slice(),
        [AssembleError::SlotConflict { .. }]
    ));
}

#[test]
fn dependency_cycle_is_rejected() {
    let a = TestModule {
        m: Manifest::new("a").after([ModuleName("b")]),
    };
    let b = TestModule {
        m: Manifest::new("b").after([ModuleName("a")]),
    };
    let err = Assembler::new()
        .with(Box::new(a))
        .with(Box::new(b))
        .seal()
        .unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| matches!(e, AssembleError::DependencyCycle(_)))
    );
}

#[test]
fn unknown_dependency_is_rejected() {
    let a = TestModule {
        m: Manifest::new("a").after([ModuleName("missing")]),
    };
    let err = Assembler::new().with(Box::new(a)).seal().unwrap_err();
    assert!(matches!(
        err.errors.as_slice(),
        [AssembleError::UnknownDependency(ModuleName("missing"))]
    ));
}

mod product {
    cicada_web::declare_module!(
        "business:product",
        owns: ["biz_product"],
        perms: ["business:product:add", "business:product:list"]
    );
}

#[test]
fn declare_module_macro_produces_valid_module() {
    let app = Assembler::new()
        .with(Box::new(product::Module))
        .seal()
        .unwrap();
    assert_eq!(
        app.names().collect::<Vec<_>>(),
        vec![ModuleName("business:product")]
    );
    assert_eq!(app.perm_count(), 2);
}

mod product_with_routes {
    cicada_web::declare_module!(
        "business:product-routes",
        routes: routes,
        owns: ["biz_product"],
        perms: ["business:product:add"]
    );

    fn routes(_state: cicada_web::AppState) -> cicada_web::Routes {
        cicada_web::Routes::new()
    }
}

#[test]
fn declare_module_accepts_routes_fn() {
    let app = Assembler::new()
        .with(Box::new(product_with_routes::Module))
        .seal()
        .unwrap();
    assert_eq!(app.perm_count(), 1);
}

/// 声明式菜单（模块边界）：模块只声明，落库交给属主的 `MenuRegistry`。
mod product_with_menus {
    use cicada_web::{Manifest, MenuNode, Permission};

    pub struct Module;

    impl cicada_web::Module for Module {
        fn manifest(self: Box<Self>) -> Manifest {
            Manifest::new("business:product-menu")
                .perms([Permission::new("business:product:view")])
                .menus([
                    MenuNode::menu(
                        "商品管理",
                        Permission::new("business:product:view"),
                        "/business/product",
                        Some("系统管理"),
                    ),
                    MenuNode::button("商品查询", Permission::new("business:product:list")),
                ])
        }
    }
}

#[test]
fn declared_menus_are_carried_out_as_specs() {
    let app = Assembler::new()
        .with(Box::new(product_with_menus::Module))
        .seal()
        .unwrap();
    let menus = app.declared_menus();
    assert_eq!(menus.len(), 2, "菜单与按钮都应被组装根取走");
    assert_eq!(menus[0].module, "business:product-menu");
    assert_eq!(menus[0].path.as_deref(), Some("/business/product"));
    assert_eq!(menus[0].parent.as_deref(), Some("系统管理"));
    // 按钮不带 path，父级留空由属主按 `:view` 约定推断。
    assert_eq!(
        menus[1].permission.as_deref(),
        Some("business:product:list")
    );
    assert!(menus[1].path.is_none() && menus[1].parent.is_none());
}
