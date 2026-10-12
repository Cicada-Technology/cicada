#!/usr/bin/env python3
"""依赖分层检查（「依赖规则（CI 强制）」）。

为什么要脚本而不是单元测试：这条规则管的是 **crate 之间能不能连边**，与任何一段 Rust
代码的行为无关；放在测试里要等编译完整 workspace 才跑得动，放在 CI 里则是秒级门禁。
本地同样一条命令：`python3 deploy/check_layering.py`。

规则（与分层架构的层图、依赖规则的 1–4 条一致）：

  L0 kernel      不依赖任何其他 cicada crate
  L1 infra/ports 只依赖 L0
  L2 领域        system / monitor / notify / jobs 只依赖 L0/L1，彼此之间不得相连
  L3 能力        auth / rbac / audit / storage / codegen 只依赖 L0/L1
  L4 接入        web / worker 可用 L0–L3（路由要读各层的实体与端口）
  数据层         migrations 只依赖 L0/L1（不许反向依赖领域或能力层）
  宏             macros 不依赖任何 cicada crate
  例外           `crates/all`（门面）与 `apps/*`（组装根）允许连所有层；testkit 不得连 apps

只检查 `[dependencies]`：dev-dependencies 与 build-dependencies 不进产物，
测试为了搭夹具跨层依赖是允许的。
"""

import os
import re
import sys

# crate 名 -> 层号。数字越大越靠外，规则是「只能向下依赖」。
LAYERS = {
    "kernel": 0,
    "infra": 1,
    "ports": 1,
    "macros": 0,  # 过程宏，与 kernel 同级（不依赖任何实现）
    "system": 2,
    "monitor": 2,
    "notify": 2,
    "jobs": 2,
    "auth": 3,
    "rbac": 3,
    "audit": 3,
    "storage": 3,
    "codegen": 3,
    "migrations": 1,  # 数据层：与 L1 同高，只许向下
    "web": 4,
    "worker": 4,
    "testkit": 4,
    "all": 9,  # 门面：允许连所有层（模板化机制机制 3）
    "server": 9,  # 组装根
    "cli": 9,
}

# 同层之间也不许连边的组（依赖规则规则 3：L2 领域模块彼此不得依赖）。
SAME_LAYER_ISOLATED = {2}

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEP_SECTION = re.compile(r"^\[dependencies\]$(.*?)(?=^\[|\Z)", re.M | re.S)
DEP_LINE = re.compile(r'^(cicada-[a-z0-9-]+|cicada)\s*=', re.M)


def crate_dir_and_name(path):
    """`crates/web/Cargo.toml` -> ('web', 'crates/web')；`apps/cli` -> ('cli', ...)。"""
    pkg = os.path.basename(os.path.dirname(path))
    return pkg, os.path.dirname(path)


def internal_deps(manifest):
    text = open(manifest, encoding="utf-8").read()
    section = DEP_SECTION.search(text)
    if not section:
        return []
    names = set()
    for line in DEP_LINE.findall(section.group(1)):
        name = line.replace("cicada-", "") or "all"
        names.add("all" if name == "cicada" else name)
    return sorted(names)


def manifests():
    for parent in ("crates", "apps"):
        base = os.path.join(ROOT, parent)
        for pkg in sorted(os.listdir(base)):
            manifest = os.path.join(base, pkg, "Cargo.toml")
            if os.path.isfile(manifest):
                yield manifest
    yield os.path.join(ROOT, "migrations", "Cargo.toml")


def check():
    problems = []
    for manifest in manifests():
        pkg, _ = crate_dir_and_name(manifest)
        if pkg not in LAYERS:
            problems.append(f"{pkg}: 未登记在分层表里（新增 crate 要同时更新分层架构与本脚本）")
            continue
        for dep in internal_deps(manifest):
            if dep not in LAYERS:
                problems.append(f"{pkg} -> {dep}: 依赖了未登记的 crate {dep}")
                continue
            mine, theirs = LAYERS[pkg], LAYERS[dep]
            if mine == 9:
                continue  # 门面 / 组装根：随便连
            if theirs > mine:
                problems.append(
                    f"{pkg}(L{mine}) -> {dep}(L{theirs}): 反向依赖，依赖规则规则 1/2/3 不允许"
                )
            elif theirs == mine and mine in SAME_LAYER_ISOLATED and dep != pkg:
                problems.append(
                    f"{pkg} -> {dep}: 同为 L{mine} 的模块不得互相依赖具体实现，"
                    "跨模块协作走 ports 端口或领域事件（依赖规则规则 3/6）"
                )
    return problems


def main():
    problems = check()
    if problems:
        print("依赖分层检查失败：", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1
    print(f"依赖分层检查通过：{len(list(manifests()))} 个 crate")
    return 0


if __name__ == "__main__":
    sys.exit(main())
