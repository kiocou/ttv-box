# OpenList 集成说明

TTV Box 将除光鸭云盘外的来源统一交给 OpenList 管理。Rust 后端启动本地 OpenList Sidecar，前端只调用 Tauri `openlist_*` 命令，不直接访问 OpenList，也不保存账号密码或 Token。

开发环境可设置：

```text
TTV_OPENLIST_BIN=D:\path\to\openlist.exe
TTV_OPENLIST_URL=http://127.0.0.1:5244
TTV_OPENLIST_AUTO_START=1
```

当前 Windows x64 sidecar：

- OpenList `v4.2.5`
- Commit `cc87e88`
- 官方 Windows x64 ZIP SHA-256：`12a627f91d5832e73f2d4045a9f6116435e08f423c4f2698ac3e25fddb946a76`
- 随包 `openlist.exe` SHA-256：`4204D345363F88A5AC0A67214DECB79C6F5A1242592BE7FA53FB4336FB96B3BC`
- 文件：`src-tauri/resources/openlist/openlist.exe`

OpenList 以 AGPL-3.0-or-later 发布；随安装包分发完整许可证文件 `src-tauri/resources/openlist/LICENSE-AGPL-3.0.txt`，并记录上游许可证地址和对应版本源码地址。缺少 sidecar 时应用会显示“未找到 OpenList 可执行文件”，不会伪造在线状态。

- AGPL-3.0 license: https://www.gnu.org/licenses/agpl-3.0.html
- v4.2.5 source/release: https://github.com/OpenListTeam/OpenList/releases/tag/v4.2.5
