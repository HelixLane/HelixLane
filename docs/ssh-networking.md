# SSH networking / SSH 网络

Open **SSH networking / jump hosts / forwarding** in a Terminal or SFTP connection pane to choose a jump resource, transport policy, or local/remote/SOCKS5 forwarding. Local and SOCKS listeners default to loopback. Bind port `0` requests an assigned port; the job shows the actual listening address.

**Shared** reuses the complete captured route and final SSH transport. Each terminal has its own screen and channel. **Single channel** rejects additional channels while any PTY, SFTP, exec, or forwarding channel occupies that transport. **Independent connection** explicitly opens another transport. Closing a view does not stop its sessions or forwards; use the job's **Stop** or the connection's **Disconnect**.

Remote **Stop** first ends the job's queued and active streams and asks the server to remove its listener. If the server rejects cancellation or does not confirm it within five seconds, HelixLane closes the owning shared SSH connection so the remote listener is not left without a cleanup owner. The job retains the cancellation failure reason. This also ends terminals, SFTP, exec tasks and other forwards on that shared transport. Choose **Independent connection** beforehand when that work needs isolation; other independent transports stay open. **Disconnect** always ends the owning transport and its shared work.

在终端或 SFTP 连接面板中展开 **SSH 网络 / 跳板 / 转发**，可选择跳板资源、传输策略，以及本地、远程或 SOCKS5 转发。本地和 SOCKS 监听默认使用回环地址。监听端口 `0` 表示请求自动分配，任务状态显示实际监听地址。

**共享通道**复用完整捕获路由及最终 SSH 传输，每个终端拥有独立屏幕和通道。**单通道**在 PTY、SFTP、exec 或转发通道占用传输期间拒绝额外通道。**独立连接**明确建立另一条传输。关闭视图不会停止会话或转发；请使用任务的**停止**或连接的**断开**。

远程**停止**会先结束该任务已排队及活动的流，再请求服务器移除监听端口。如果服务器拒绝取消，或五秒内未确认，HelixLane 将关闭所属共享 SSH 连接，避免监听端口失去清理所有者；任务保留具体失败原因。这会同时结束该共享传输上的终端、SFTP、exec 和其他转发。需要隔离的任务应事先选择**独立连接**，其他独立传输保持打开。**断开**始终结束所属传输及共享任务。
