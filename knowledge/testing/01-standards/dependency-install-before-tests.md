---
id: dependency-install-before-tests
title: 运行测试/lint 前先装依赖（含 dev/test extras，一步到位不返工）
domain: testing
category: 01-standards
difficulty: intermediate
tags: [dependency, dev-extras, test-tooling, uv, pip, poetry, pdm, npm, pytest, ruff, one-pass, no-retry, 依赖安装, 测试依赖, 开发依赖, 一步到位, 商业级]
quality_score: 95
last_updated: 2026-07-02
---
# 运行测试/lint 前先装依赖（含 dev/test extras）

> 这条规范修的是一个高频浪费：自动化跑流程时，直接就去执行 `uv run python -m pytest -q` / `uv run ruff check`，结果环境里根本没装 `pytest`、`ruff`，报 `No module named pytest`，然后才回头 `uv sync --extra dev` 再重试——白白多走一整轮。
> 核心结论：**跑测试/lint 之前，先把项目依赖（含 dev/test extras）一步装好，再跑测试。** 测试运行时报"缺某个模块/命令"几乎都是**你漏了装依赖这一步**，不是测试真的挂了。

## 1. 铁律：先装依赖，再跑测试/lint

- 跑任何测试或 lint 命令**之前**，先确认依赖已安装，而且**包含 dev/test extras**（`pytest`、`ruff`、`mypy`、`eslint`、`jest` 这些工具都在开发依赖里，不在运行依赖里）。
- 安装要**一步到位**：一条 sync/install 命令把运行依赖 + 开发/测试依赖一起装好，然后再跑测试。不要"先跑、报错、再补装、再重试"——那是把一步拆成三步。
- 看到 `No module named pytest` / `ModuleNotFoundError` / `pytest: command not found` / `ruff: not found`：**这是漏装依赖，不是测试失败**。正确动作是回到"装依赖（含 dev/test）"这一步，而**不是**原样重试同一条命令、也不是去改测试代码。

## 2. 各生态的准确命令

### Python · uv（本条最容易踩的坑）

- **默认的 `uv sync` 不装 dev/test extras**。只跑 `uv sync` 之后再 `uv run pytest` / `uv run ruff`，工具找不到，就会报 `No module named pytest`。
- 正确做法（任选其一，按项目声明方式）：
  - `uv sync --extra dev`（`pyproject.toml` 里用 `[project.optional-dependencies].dev` 声明时）
  - `uv sync --all-extras`（一次装齐所有 extras）
  - `uv sync --group dev`（用 dependency-groups 声明时）
- 装好后再 `uv run pytest -q` / `uv run ruff check`。

### Python · pip / venv

- `pip install -e '.[dev]'`（`pyproject.toml` / `setup.cfg` 声明了 `dev` extra 时；注意引号，zsh 下 `.[dev]` 会被当成通配符）
- 或 `pip install -r requirements-dev.txt`（有独立开发依赖清单时）

### Python · poetry / pdm

- poetry：`poetry install --with dev`（`--with` 装 dev group；`--only` 会**只**装某组，别误用）
- pdm：`pdm install -G dev`（`-G/--group` 指定开发组）

### Node · npm / pnpm / yarn

- `npm ci`（按 lockfile 精确安装，**包含 devDependencies**——`jest`/`eslint`/`vitest` 就在这里）
- pnpm：`pnpm install --frozen-lockfile`；yarn：`yarn install --frozen-lockfile`
- 注意：`npm ci --omit=dev` / `NODE_ENV=production` 会**跳过** devDependencies，那样测试工具就装不上——跑测试的环境不要用生产安装模式。

## 3. 判断准则（把"缺模块报错"归类对）

- 测试运行时出现"缺模块/缺命令"（`No module named X`、`X: command not found`、`is not recognized`）→ **依赖问题**，去装依赖，不是测试问题。
- 缺的是**测试/lint 工具本身**（pytest/ruff/mypy/eslint/jest…）→ 是 **dev/test extras 没装**，用上面对应生态的命令补齐。
- 缺的是**业务依赖**（如 `requests`、`lodash`）→ 是运行依赖没装或没声明，正常 install/add 后再跑。
- 只有在**依赖确已装齐**的前提下，测试仍然红，才是真正的测试失败，这时才去看断言与实现。

## 4. 反模式（不要这样做）

- 直接 `uv run pytest` 前不 sync，报错后才 `uv sync --extra dev` 再重试——多走一轮。
- 报 `No module named pytest` 后去**改测试代码 / 加 skip / 装错包**，而不是装 dev 依赖。
- 用生产安装模式（`--omit=dev` / `--only main` / `NODE_ENV=production`）准备测试环境，导致测试工具缺失。
- 把 `uv sync --extra dev` 当成"报错后的补救"，而不是"跑测试前的既定第一步"。
