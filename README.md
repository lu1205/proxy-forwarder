# 接口请求转发客户端

这是一个基于 Tauri 2 的 Windows 桌面客户端。应用启动后会在本机开启一个 HTTP 服务，外部程序调用本地接口即可让客户端按参数描述发起请求，并把目标接口结果返回。

应用默认后台启动，不主动显示主窗口；启动后会出现在系统托盘中。双击托盘图标或在托盘菜单点击“显示窗口”可以打开主窗口，点击窗口关闭按钮会隐藏到托盘并继续提供转发服务，只有托盘菜单中的“退出”会结束程序。

## 本地接口

- 健康检查：`GET http://127.0.0.1:39291/health`
- 请求转发：`POST http://127.0.0.1:39291/forward`
- 文件数据获取：`POST http://127.0.0.1:39291/file-data`

请求示例：

```json
{
  "method": "POST",
  "url": "https://example.com/api",
  "headers": {
    "Authorization": "Bearer token"
  },
  "query": {
    "page": "1"
  },
  "body": {
    "name": "demo"
  },
  "timeoutMs": 30000
}
```

响应示例：

```json
{
  "status": 200,
  "headers": {
    "content-type": "application/json"
  },
  "body": "{\"ok\":true}",
  "bodyJson": {
    "ok": true
  },
  "elapsedMs": 132
}
```

文件数据获取请求示例：

```json
{
  "fileUrl": "https://example.com/file.pdf",
  "headers": {
    "Authorization": "Bearer token"
  },
  "timeoutMs": 60000
}
```

也可以使用 `url` 或 `link` 字段传文件链接。该接口会使用 `GET` 请求目标文件地址，并把目标文件的原始二进制内容直接作为响应体返回；如果目标响应包含 `Content-Type` 或 `Content-Disposition`，会同步返回给调用方。

## 开发运行

```powershell
cd src-tauri
cargo run
```

首次运行需要下载 Rust 依赖。打包 Windows 安装包可在 `src-tauri` 目录执行：

```powershell
cargo tauri build
```

生成的安装包位于：

```text
src-tauri\target\release\bundle\nsis\Proxy Forwarder_0.1.0_x64-setup.exe
```

安装包会创建开始菜单快捷方式，并在安装完成后写入当前用户的开机自启项。应用自身每次启动时也会确认自启已开启；卸载时会清理对应的开机自启注册表项。
