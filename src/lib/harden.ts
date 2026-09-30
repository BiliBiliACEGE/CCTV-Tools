/**
 * 打包产物的外壳加固：收紧 WebView 里的浏览器入口。
 *
 * 三层防护，缺一不可：
 *  1. 引擎层（最强）：`src-tauri/Cargo.toml` 里 `tauri` **没有**开启 `devtools` feature。
 *     release 构建下 `tauri-runtime-wry` 那段 `with_devtools(...)` 调用被 `cfg` 整个编译掉，
 *     于是落到 wry 的默认值 `devtools: false`，最终写成 WebView2 的
 *     `AreDevToolsEnabled(FALSE)`——F12 / Ctrl+Shift+I 在浏览器内核层就无效，
 *     根本没有"打开开发者工具"这条路径。
 *  2. 权限层：`capabilities/default.json` 显式 deny 掉 `internal-toggle-devtools`，
 *     即便将来有人误开了 `devtools` feature，前端 IPC 也调不动。
 *  3. 界面层（本文件）：拦掉右键菜单与几个会打乱应用状态的浏览器快捷键。
 *
 * 只在非开发构建里生效：`import.meta.env.DEV` 为真时直接返回，
 * 免得 `npm run app` 调试时连"检查元素"都用不了。
 */

/** 命中即吞掉的快捷键。注意不要误伤 Ctrl+C / Ctrl+V / Ctrl+A —— 界面里有输入框。 */
function isBlockedShortcut(event: KeyboardEvent): boolean {
  const key = event.key.toLowerCase();
  const ctrl = event.ctrlKey || event.metaKey;

  // F12：唯一的单键入口
  if (key === 'f12') return true;

  // 开发者工具的三种组合键
  if (ctrl && event.shiftKey && (key === 'i' || key === 'j' || key === 'c')) return true;

  // Ctrl+U 查看源码 / Ctrl+R 重载 / Ctrl+P 打印 / Ctrl+S 保存页面
  if (ctrl && (key === 'u' || key === 'r' || key === 'p' || key === 's')) return true;

  // F5 重载
  if (key === 'f5') return true;

  return false;
}

/** 安装外壳加固。要在渲染之前调用一次。 */
export function installShellHardening(): void {
  if (import.meta.env.DEV) return;

  // 右键菜单：WebView2 只在 DOM 的 contextmenu 事件未被取消时才弹原生菜单，
  // 捕获阶段拦下即可；顺带把图片"另存为"、链接"在新窗口打开"一并掐掉。
  document.addEventListener('contextmenu', (event) => event.preventDefault(), true);

  // 快捷键走捕获阶段，避免被输入框、hls.js 播放器等先消费掉
  window.addEventListener(
    'keydown',
    (event) => {
      if (!isBlockedShortcut(event)) return;
      event.preventDefault();
      event.stopPropagation();
    },
    true,
  );

  // 往窗口里拖文件会让 WebView 直接跳转到该文件（等于换掉了整个界面），一起挡掉
  document.addEventListener('dragstart', (event) => event.preventDefault(), true);
  document.addEventListener('dragover', (event) => event.preventDefault(), true);
  document.addEventListener('drop', (event) => event.preventDefault(), true);
}
