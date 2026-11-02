use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}
impl Default for Language {
    fn default() -> Self {
        if std::env::var("LANG").unwrap_or_default().starts_with("zh") {
            Self::Chinese
        } else {
            Self::English
        }
    }
}
impl Language {
    pub fn text(self, key: &str) -> &'static str {
        let zh = self == Self::Chinese;
        match key {
            "tmux" => "tmux",
            "plugin" => {
                if zh {
                    "资源插件"
                } else {
                    "Resource plugin"
                }
            }
            "plugin_title" => {
                if zh {
                    "资源概览 · JavaScript 面板"
                } else {
                    "Resource overview · JavaScript panel"
                }
            }
            "plugin_view" => {
                if zh {
                    "视图"
                } else {
                    "View"
                }
            }
            "plugin_enable" => {
                if zh {
                    "启用示例插件"
                } else {
                    "Enable example plugin"
                }
            }
            "plugin_unload" => {
                if zh {
                    "卸载"
                } else {
                    "Unload"
                }
            }
            "plugin_retry" => {
                if zh {
                    "重新加载"
                } else {
                    "Reload"
                }
            }
            "plugin_disabled" => {
                if zh {
                    "插件已停用。启用后仅可读取当前资源的名称、主机和端口。"
                } else {
                    "Plugin disabled. Enabling grants the current resource name, host and port only."
                }
            }
            "plugin_permissions" => {
                if zh {
                    "权限：只读资源概要。文件、进程、网络、凭据和设置均不可访问。"
                } else {
                    "Permissions: resource summary only. Files, processes, network, credentials and settings are denied."
                }
            }
            "plugin_running" => {
                if zh {
                    "已启用 · 原生脚本视图"
                } else {
                    "Enabled · native script view"
                }
            }
            "plugin_failed" => {
                if zh {
                    "插件出错，请重新加载或卸载。"
                } else {
                    "Plugin failed. Reload or unload to recover."
                }
            }
            "recovered_layout" => {
                if zh {
                    "部分布局无法恢复，已保留并重新打开视图。"
                } else {
                    "Part of the layout could not be restored. Saved views were preserved and reopened."
                }
            }
            "invalid_resource" => {
                if zh {
                    "请填写名称、主机、用户名和有效端口。"
                } else {
                    "Enter a name, host, username and valid port."
                }
            }
            "global" => {
                if zh {
                    "全局"
                } else {
                    "Global"
                }
            }
            "recovered_settings" => {
                if zh {
                    "设置损坏，已重置布局并保留资源。"
                } else {
                    "Settings were invalid. Layout reset; saved resources preserved."
                }
            }
            "recovered_resource" => {
                if zh {
                    "损坏的资源记录已归档以便恢复。"
                } else {
                    "A damaged resource record was archived for recovery."
                }
            }
            "recovered_profile" => {
                if zh {
                    "不完整的连接配置已重置，请检查设置。"
                } else {
                    "An incomplete connection profile was reset. Review its settings."
                }
            }
            "settings_error" => {
                if zh {
                    "无法保存设置"
                } else {
                    "Could not save settings"
                }
            }
            "views" => {
                if zh {
                    "视图"
                } else {
                    "views"
                }
            }
            "run_policy" => {
                if zh {
                    "运行规则"
                } else {
                    "Run policy"
                }
            }
            "workbench" => {
                if zh {
                    "远程工作台"
                } else {
                    "Remote workbench"
                }
            }
            "settings" => {
                if zh {
                    "外观与语言"
                } else {
                    "Appearance & language"
                }
            }
            "move_to" => {
                if zh {
                    "移动到"
                } else {
                    "Move to"
                }
            }
            "toggle_policy" => {
                if zh {
                    "切换后台 / 可见时运行"
                } else {
                    "Toggle Background / While visible"
                }
            }
            "group" => {
                if zh {
                    "分组"
                } else {
                    "Group"
                }
            }
            "tags" => {
                if zh {
                    "标签"
                } else {
                    "Tags"
                }
            }
            "notes" => {
                if zh {
                    "备注"
                } else {
                    "Notes"
                }
            }
            "system" => {
                if zh {
                    "系统"
                } else {
                    "System"
                }
            }
            "dark" => {
                if zh {
                    "深色"
                } else {
                    "Dark"
                }
            }
            "light" => {
                if zh {
                    "浅色"
                } else {
                    "Light"
                }
            }
            "four_hosts" => {
                if zh {
                    "四机分屏"
                } else {
                    "Four hosts"
                }
            }
            "confirm_delete" => {
                if zh {
                    "确认删除"
                } else {
                    "Confirm delete"
                }
            }
            "resources" => {
                if zh {
                    "资源"
                } else {
                    "Resources"
                }
            }
            "services" => {
                if zh {
                    "服务"
                } else {
                    "Services"
                }
            }
            "terminal" => {
                if zh {
                    "终端"
                } else {
                    "Terminal"
                }
            }
            "sftp" => {
                if zh {
                    "文件传输"
                } else {
                    "SFTP"
                }
            }
            "transfers" => {
                if zh {
                    "传输任务"
                } else {
                    "Transfers"
                }
            }
            "information" => {
                if zh {
                    "资源信息"
                } else {
                    "Information"
                }
            }
            "connection_profile" => {
                if zh {
                    "连接配置"
                } else {
                    "Connection profile"
                }
            }
            "workspace_status" => {
                if zh {
                    "本地工作区"
                } else {
                    "Local workspace"
                }
            }
            "follow" => {
                if zh {
                    "跟随"
                } else {
                    "Follow"
                }
            }
            "fixed" => {
                if zh {
                    "固定"
                } else {
                    "Fixed"
                }
            }
            "background" => {
                if zh {
                    "后台运行"
                } else {
                    "Background"
                }
            }
            "visible" => {
                if zh {
                    "可见时运行"
                } else {
                    "While visible"
                }
            }
            "new" => {
                if zh {
                    "新建资源"
                } else {
                    "New resource"
                }
            }
            "edit" => {
                if zh {
                    "编辑资源"
                } else {
                    "Edit resource"
                }
            }
            "save" => {
                if zh {
                    "保存"
                } else {
                    "Save"
                }
            }
            "cancel" => {
                if zh {
                    "取消"
                } else {
                    "Cancel"
                }
            }
            "empty" => {
                if zh {
                    "选择资源以打开终端或文件视图"
                } else {
                    "Choose a resource to open a terminal or file view"
                }
            }
            "no_connection" => {
                if zh {
                    "尚未建立连接。此视图仅展示布局，不会执行远程操作。"
                } else {
                    "No connection opened. This view demonstrates layout; no remote operation is performed."
                }
            }
            "name" => {
                if zh {
                    "名称"
                } else {
                    "Name"
                }
            }
            "host" => {
                if zh {
                    "主机"
                } else {
                    "Host"
                }
            }
            "user" => {
                if zh {
                    "用户名"
                } else {
                    "Username"
                }
            }
            "port" => {
                if zh {
                    "端口"
                } else {
                    "Port"
                }
            }
            "no_tasks" => {
                if zh {
                    "没有传输任务。关闭视图不会取消后台任务。"
                } else {
                    "No transfers. Closing a view does not cancel background tasks."
                }
            }
            "by_resource" => {
                if zh {
                    "按资源"
                } else {
                    "By resource"
                }
            }
            "by_type" => {
                if zh {
                    "按类型"
                } else {
                    "By type"
                }
            }
            "delete" => {
                if zh {
                    "删除资源"
                } else {
                    "Delete resource"
                }
            }
            "local" => {
                if zh {
                    "本地文件"
                } else {
                    "Local files"
                }
            }
            "remote" => {
                if zh {
                    "远程文件"
                } else {
                    "Remote files"
                }
            }
            "offline" => {
                if zh {
                    "未连接"
                } else {
                    "Disconnected"
                }
            }
            "center" => {
                if zh {
                    "中央"
                } else {
                    "Center"
                }
            }
            "left" => {
                if zh {
                    "左侧"
                } else {
                    "Left"
                }
            }
            "right" => {
                if zh {
                    "右侧"
                } else {
                    "Right"
                }
            }
            "top" => {
                if zh {
                    "顶部"
                } else {
                    "Top"
                }
            }
            "bottom" => {
                if zh {
                    "底部"
                } else {
                    "Bottom"
                }
            }
            "split_h" => {
                if zh {
                    "左右分屏"
                } else {
                    "Split horizontal"
                }
            }
            "split_v" => {
                if zh {
                    "上下分屏"
                } else {
                    "Split vertical"
                }
            }
            "close" => {
                if zh {
                    "关闭视图"
                } else {
                    "Close view"
                }
            }
            _ => "",
        }
    }
}

impl Language {
    pub fn ssh_error(self, error: &str) -> &str {
        if self != Self::Chinese {
            return error;
        }
        match error {
            "Terminal input not sent: shell is finished or unavailable" => {
                "终端输入未发送：Shell 已结束或不可用"
            }
            "Terminal input not sent: paste exceeds 64 KiB" => "终端输入未发送：粘贴超过 64 KiB",
            "Terminal input not sent: outbound queue is full or closed" => {
                "终端输入未发送：发送队列已满或已关闭"
            }
            "SSH connection or host-key verification failed" => "SSH 连接或主机身份验证失败",
            "Connection timed out" => "连接超时",
            "Authentication rejected" => "认证被拒绝",
            "Authentication transport failed" => "认证连接失败",
            "System keychain unavailable; credential was not saved" => {
                "系统钥匙串不可用，凭据未保存"
            }
            "System keychain unavailable or credential missing" => "系统钥匙串不可用或找不到凭据",
            "Cannot read private key or incorrect passphrase" => "无法读取私钥或口令不正确",
            "Host-key store is corrupt; repair it before connecting" => {
                "主机密钥记录已损坏，请修复后重试"
            }
            "Host identity changed while approval was pending" => {
                "等待确认期间主机身份再次发生变化"
            }
            "Server refused PTY or shell (SFTP-only hosts need the SFTP view)" => {
                "服务器拒绝终端或 shell；仅支持 SFTP 的主机请使用 SFTP 视图"
            }
            "Shell request failed (this may be an SFTP-only host)" => {
                "无法打开 shell，此主机可能仅支持 SFTP"
            }
            "PTY request failed" => "终端请求失败",
            "Server refused a session channel" => "服务器拒绝会话通道",
            "Terminal input failed" => "终端输入失败",
            "Terminal resize failed" => "终端尺寸更新失败",
            "Terminal response failed" => "终端响应失败",
            "Single-channel connection is occupied. Close its channel or explicitly choose Independent connection." => {
                "单通道连接已被占用。请关闭占用通道，或明确选择独立连接。"
            }
            "Jump route contains a cycle" => "跳板链包含循环",
            "Jump route exceeds eight hops" => "跳板链超过八跳限制",
            "Jump route has not been captured" => "尚未捕获跳板链配置",
            "Invalid captured jump route" => "捕获的跳板链配置无效",
            "SSH agent unavailable" => "SSH Agent 不可用",
            "SSH agent identity request failed" => "SSH Agent 身份请求失败",
            "SSH agent signing failed" => "SSH Agent 签名失败",
            "Authentication response count mismatch" => "认证回答数量不匹配",
            "Authentication prompt timed out" => "认证提示超时",
            "Authentication timed out" => "认证超时",
            "Interactive authentication failed" => "交互式认证失败",
            "Interactive authentication limits exceeded" => "交互式认证超出次数或大小限制",
            "Invalid forwarding destination" => "转发目标无效",
            "Cannot bind forwarding listener" => "无法绑定转发监听地址",
            "Forward channel timed out" => "转发通道超时",
            "Forward destination refused" => "转发目标拒绝连接",
            "Remote forwarding refused" => "远程转发被拒绝",
            "Remote forward cancellation failed; shared connection closed to remove listener" => {
                "远程转发取消失败；已关闭共享连接以移除监听端口"
            }
            "Remote forward cancellation timed out; shared connection closed to remove uncertain listener" => {
                "远程转发取消超时；已关闭共享连接以移除状态不明的监听端口"
            }
            "Remote forward cancellation failed" => "远程转发取消失败",
            "Remote forward cancellation timed out" => "远程转发取消超时",
            "Remote listener is already registered" => "远程监听地址已注册",
            "Jump hosts are not configured for this connection" => "此连接尚未配置跳板主机",
            "Key negotiation failed" => "密钥协商失败",
            "Keychain task failed" => "钥匙串操作失败",
            "Private-key task failed" => "私钥加载失败",
            "Missing keychain reference" => "缺少钥匙串引用",
            _ => error,
        }
    }
}

impl Language {
    pub fn file_error(self, error: &str) -> &str {
        if self != Self::Chinese {
            return error;
        }
        if error.starts_with("Incompatible destination type") {
            return "目标类型不兼容，原目标保持不变";
        }
        if error.starts_with("Source changed") {
            return "源文件已变化，请重新选择传输";
        }
        if error.starts_with("Target changed") {
            return "目标已变化，请重新确认操作";
        }
        if error.starts_with("Remote file changed") {
            return "远程文件已变化，请重新下载或解决冲突";
        }
        match error {
            "Source missing" => "源文件不存在",
            "Invalid path" => "路径无效",
            "Select an entry first" => "请先选择文件或目录",
            "Select one entry" => "请选择一个文件或目录",
            "Target exists" => "目标已存在",
            "Directory conflict" => "目标目录类型冲突",
            "Invalid directory name" => "目录名称无效",
            "Select one entry and enter a valid name" => "请选择一个文件或目录并输入有效名称",
            "Enter octal permissions, e.g. 644" => "请输入八进制权限，例如 644",
            "Invalid permissions" => "权限无效",
            "Refusing to follow symlink" => "为安全起见，不跟随符号链接",
            "Refusing to follow symlink or non-file" => "不能读取符号链接或非常规文件",
            "Only regular files can be edited" => "只能编辑常规文件",
            "Choose an external editor executable" => "请选择外部编辑器可执行文件",
            "Unsafe editor temporary directory" => "编辑器临时目录不安全",
            "External edit limit is 32 MiB" => "外部编辑文件最大支持 32 MiB",
            "No saved changes detected" => "未检测到保存的更改",
            "SFTP subsystem timed out" => "SFTP 子系统连接超时",
            "Invalid tmux name" => "tmux 会话名称无效",
            "tmux command failed; check installation and session availability" => {
                "tmux 命令失败，请检查是否已安装以及会话是否可用"
            }
            "Select a running target terminal instance" => "请选择正在运行的目标终端实例",
            "Target terminal connection changed" => "目标终端连接已变化",
            "Target cannot be source or a child of source" => "目标不能与源相同或位于源目录内",
            "Local staging path already exists" => "本机中转路径已存在",
            "Backup path exists" => "备份路径已存在",
            "Cancelled" => "已取消",
            "Recovery is already being validated" => "此恢复任务正在验证，请等待完成",
            "Recovery missing" => "恢复任务已被处理或忽略",
            "Task missing" => "任务不存在",
            "Task is not retryable" => "此任务不能重试",
            "Reconnect and verify the captured connection profile before recovering this task" => {
                "请按任务原连接配置重新连接并验证主机，再恢复此任务"
            }
            _ => self.ssh_error(error),
        }
    }
}
