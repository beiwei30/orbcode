# 集成

[English](../integrations.md) · [简体中文](integrations.md)

Orb Code 有两个集成边界：面向编辑器的 ACP adapter，以及面向 thin clients 的 canonical
app-server 协议。即使底层许多 session/tool 操作稳定，这两层仍是实验性的。

## Agent Client Protocol（ACP）

在 stdio 上运行 ACP v1：

```bash
orbcode acp
```

它把 ACP session 创建/加载、prompts、streamed text/thought/tool updates、model selection、
thought controls、permissions、history restoration 和兼容 MCP setup 映射到 TUI 共用的
in-process app server。

ACP 只声明能够闭环完成的 capabilities。稳定 permission request 可映射单个选择，但不会声明
Orb Code 更丰富的 canonical 多问题 schema。image/resource/audio prompt content 以当前
capability matrix 为准，不会乐观接受。构建客户端前阅读 [ACP 支持矩阵](acp-support.md)。

### Zed

已测试的方式是把 binary 作为 custom agent 启动，在 Zed agent 配置中指向 `orbcode acp`。
冒烟指南覆盖 lifecycle、streaming、history restoration、model/thought controls、permissions、
MCP、content types 和 shutdown：[ACP with Zed](acp-zed-smoke.md)。其中版本是已记录基线，
不是对所有未来 Zed/ACP 版本的永久承诺。

## App-server 协议

协议版本 `1.0`，包含 request/response envelopes、stream-event notifications，以及 server 发起的
permission、MCP trust、interactive question requests。client 初始化时声明 streaming、
experimental methods、persistent goals、interactive questions capabilities。

stable request families 覆盖 session bootstrap/list/fork/rename/rewind、turn submit/steer/cancel、
permission rules、settings、context/usage、MCP、tools、auth、diagnostics。实验族包含 persistent
goals、部分 session controls、background tasks、dynamic workflows。初始化会返回 stable 与
experimental method lists；client 不应假设实验方法跨版本不变。

### 启动 server

产品界面仍实验，因此 `serve` 刻意从顶层 help 隐藏：

```bash
orbcode serve --stdio
orbcode serve --socket /tmp/orbcode.sock --auth-token "$TOKEN"
orbcode serve --websocket 127.0.0.1:8080 --auth-token "$TOKEN" \
  --allowed-origin https://client.example
```

socket/WebSocket listener 支持依次重连，同一时间一个 active client，并要求 token auth。
WebSocket 还可拒绝不匹配的 `Origin`。不要把未经审计的 endpoint 绑定到公网。stdio 由父进程
拥有，隐式 trusted。

### Remote TUI

```bash
orbcode remote /tmp/orbcode.sock --token "$TOKEN"
orbcode remote ws://127.0.0.1:8080 --token "$TOKEN"
```

remote mode 不启动本地 core：sessions、tools、permissions、settings 和 streams 全来自 server。
双方都协商 capability 时，本地/远程 TUI 可监督实验性 persistent goals。

### Client libraries 与延后产品

workspace 中有 in-process、NDJSON、WebSocket、child-stdio、经审查的 OpenSSH transport 和生成的
TypeScript contracts。它们是基础设施，不是打包好的 Desktop app 或 `remote --ssh` 产品。
Desktop、SSH CLI、remote-control bridge、voice、computer use 仍 deferred，见
[延后产品决策](../plans/desktop-and-ssh-products.md)。

程序拥有单一 CLI 子进程且只需 turn/control correlation 时使用[双向 stream-JSON](stream-json.md)；
需要更长生命周期的多 session facade 且能接受实验协议时使用 app-server。
