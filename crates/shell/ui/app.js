// natpierce-keepalive — 前端逻辑
// 通过 Tauri 的 invoke 调用 Rust 后端命令

// ---------- Tauri API 安全获取 ----------
// Tauri 2 默认不注入 window.__TAURI__，需要在 tauri.conf.json 里开启
// app.withGlobalTauri。这里做容错，避免拿不到 API 时整个脚本崩掉。
function tauriApi() {
  return window.__TAURI__ || null;
}

/** 调用后端命令（带友好错误） */
async function invoke(cmd, args) {
  const api = tauriApi();
  if (!api || !api.core) {
    throw new Error('Tauri API 不可用：请在 tauri.conf.json 中开启 app.withGlobalTauri');
  }
  return api.core.invoke(cmd, args);
}

/** 隐藏当前窗口 */
async function hideWindow() {
  const api = tauriApi();
  if (!api || !api.window) return;
  await api.window.getCurrentWindow().hide();
}

/** 监听后端事件 */
async function listenEvent(name, handler) {
  const api = tauriApi();
  if (!api || !api.event) return;
  await api.event.listen(name, handler);
}

/**
 * 打开文件/目录选择对话框
 * 依赖 tauri-plugin-dialog
 */
async function pickPath({ directory = false, filters = [] } = {}) {
  const api = tauriApi();
  if (!api || !api.dialog) {
    throw new Error('文件对话框不可用：请确认已启用 tauri-plugin-dialog');
  }
  const opts = { directory, multiple: false, title: directory ? '选择目录' : '选择文件' };
  if (!directory && filters.length) opts.filters = filters;

  const picked = await api.dialog.open(opts);
  if (!picked) return null;                       // 用户取消
  return Array.isArray(picked) ? picked[0] : picked;
}

// ---------- 工具 ----------
const $ = (id) => document.getElementById(id);

function setBadge(el, text, kind) {
  el.textContent = text;
  el.className = 'badge' + (kind ? ' ' + kind : '');
}

function setStat(el, text, kind) {
  el.textContent = text;
  el.className = 'stat-value' + (kind ? ' ' + kind : '');
}

function showMsg(text, isError) {
  const el = $('save-msg');
  el.textContent = text;
  el.className = 'save-msg' + (isError ? ' err' : '');
  if (text) {
    clearTimeout(showMsg._t);
    showMsg._t = setTimeout(() => { el.textContent = ''; }, 5000);
  }
}

/** 把异常转成可读字符串 */
function errText(e) {
  if (e == null) return '未知错误';
  if (typeof e === 'string') return e;
  if (e.message) return e.message;
  try { return JSON.stringify(e); } catch (_) { return String(e); }
}

// ---------- 表单 ↔ 配置 ----------
// ⚠️ 字段名必须与 Rust 端 ConfigView 的 serde 输出一致（camelCase），
//    否则 save_config 会报 "missing field"。
const FIELDS = [
  'account',
  'exePath', 'workingDir', 'processName', 'startArgs', 'apiUrl',
  'maxClients', 'intervalSec', 'heartbeatSec', 'failThreshold',
  'restartAfterFailures', 'targetHostId', 'targetHostName',
];
const BOOLS = ['autoStartServer', 'closeServerFirst', 'keepaliveEnabled'];

// 字段名(camelCase) → DOM id：exePath → f-exe-path
const idOf = (k) => 'f-' + k.replace(/([A-Z])/g, '-$1').toLowerCase();

let currentConfig = null;
/** 最近一次配置校验结果 */
let configCheck = { ok: true, problems: [] };

/** 渲染配置警告横幅 + 更新启动按钮状态 */
function renderConfigCheck(check) {
  configCheck = check || { ok: true, problems: [] };

  const box = $('config-warning');
  const list = $('config-problems');

  if (configCheck.ok) {
    box.hidden = true;
  } else {
    box.hidden = false;
    list.innerHTML = '';
    (configCheck.problems || []).forEach((p) => {
      const li = document.createElement('li');
      li.textContent = p;
      list.appendChild(li);
    });

    // 指引按模式区分：页面访问密码只用于**开启本机服务端**，
    // 客户端模式完全不需要它。写死成一句话会误导用户去填。
    const isClient =
      document.querySelector('input[name="mode"]:checked')?.value === 'client';
    const hint = $('config-hint');
    if (hint) {
      hint.textContent = isClient
        ? '客户端模式所需：① 选择 natpierce.exe；② 指定目标识别码（可在上方「在线主机」列表点选）。改完点底部「保存配置」。'
        : '服务端模式所需：① 选择 natpierce.exe；② 填写页面访问密码（组网模式下必填）。改完点底部「保存配置」。';
    }
  }
}

/** 显示/隐藏"检测中"指示 */
function setProbing(on) {
  const el = $('probe-indicator');
  if (el) el.hidden = !on;
  const box = $('status-grid-wrap');
  if (box) box.classList.toggle('probing', on);
}

/** 重新校验配置 */
async function refreshConfigCheck() {
  try {
    const check = await invoke('check_config');
    renderConfigCheck(check);
    return check;
  } catch (e) {
    // 校验命令都失败时不阻塞用户，仅记录
    console.error('check_config 失败', e);
    return { ok: true, problems: [] };
  }
}

function fillForm(cfg) {
  currentConfig = cfg;
  FIELDS.forEach((k) => {
    const el = $(idOf(k));
    if (el) el.value = cfg[k] ?? '';
  });
  BOOLS.forEach((k) => {
    const el = $(idOf(k));
    if (el) el.checked = !!cfg[k];
  });

  // 模式
  const modeRadio = document.querySelector(`input[name="mode"][value="${cfg.mode}"]`);
  if (modeRadio) modeRadio.checked = true;
  updateModeVisibility();

  // 密码：输入框一律清空（密文从不回显），但把"到底存过没有"明确写出来 ——
  // 否则重启后框是空的，用户根本不知道自己之前填过没填过。
  $('f-page-pwd').value = '';
  $('f-conn-pwd').value = '';
  $('f-login-pwd').value = '';
  setPwdHint('login', cfg.hasLoginPassword);
  setPwdHint('page', cfg.hasPagePassword);

  $('f-autostart').checked = false; // 稍后由 autostart_status 填
}

/**
 * 更新密码框下方的保存状态提示。
 * 密文从不回显，所以只能靠这行文字告诉用户"到底存过没有"。
 */
function setPwdHint(which, saved) {
  const el = $(`${which}-pwd-hint`);
  if (!el) return;
  if (saved) {
    el.textContent = '✅ 已加密保存 · 留空则不修改，输入新密码会覆盖';
    el.className = 'pwd-hint ok';
  } else {
    el.textContent = '⚠ 尚未设置 · 必须填写并保存';
    el.className = 'pwd-hint missing';
  }
}

function readForm() {
  const view = { mode: document.querySelector('input[name="mode"]:checked')?.value || 'server' };
  FIELDS.forEach((k) => {
    const el = $(idOf(k));
    if (el) view[k] = el.value;
  });
  BOOLS.forEach((k) => {
    const el = $(idOf(k));
    if (el) view[k] = el.checked;
  });
  view.loginPassword = $('f-login-pwd')?.value || '';
  view.pagePassword = $('f-page-pwd').value;
  view.connectionPassword = $('f-conn-pwd').value;
  view.hasPagePassword = currentConfig?.hasPagePassword ?? false;
  view.hasLoginPassword = currentConfig?.hasLoginPassword ?? false;

  // 直接发 camelCase：Rust 端 ConfigView 标注了 `rename_all = "camelCase"`，
  // Tauri 命令参数即按该规则反序列化。（曾经错误地转成 snake_case，
  // 导致 `invalid args 'view' ...: missing field 'exePath'`）
  return view;
}

/**
 * 根据工作模式显示/隐藏对应区块
 * - 服务端模式：只显示服务端设置
 * - 客户端模式：只显示客户端设置
 */
function updateModeVisibility() {
  const mode = document.querySelector('input[name="mode"]:checked')?.value || 'server';
  const isClient = mode === 'client';

  document.querySelectorAll('.mode-only-server').forEach((el) => {
    el.hidden = isClient;
  });
  document.querySelectorAll('.mode-only-client').forEach((el) => {
    el.hidden = !isClient;
  });
}

// ---------- 状态渲染 ----------
function renderStatus(st) {
  // 在线主机列表只对客户端模式有意义：服务端模式不选目标主机
  const isClientMode =
    document.querySelector(`input[name="mode"]:checked`)?.value === `client`;

  // 徽章
  setBadge($('badge-daemon'),
    '保活：' + (st.daemonRunning ? '运行中' : '已停止'),
    st.daemonRunning ? 'ok' : 'warn');
  // 状态格
  setStat($('st-process'),
    st.processAlive ? '运行中' : '未运行',
    st.processAlive ? 'ok' : 'err');

  if (!st.apiReachable) {
    setStat($('st-server'), '接口未就绪', 'warn');
  } else if (st.serverRunning === true) {
    setStat($('st-server'), '已启动', 'ok');
  } else if (st.serverRunning === false) {
    setStat($('st-server'), '未启动', 'warn');
  } else {
    setStat($('st-server'), '未知', '');
  }

  // 客户端模式：显示与目标主机的连接状态
  // （unknown 表示本次探测没拿到连接状态，不等于"没连上"）
  switch (st.clientLink) {
    case 'connected':    setStat($('st-link'), '已连接', 'ok');   break;
    case 'failed':       setStat($('st-link'), '连接被拒', 'err'); break;
    case 'disconnected': setStat($('st-link'), '已断开', 'warn'); break;
    case 'unknown':      setStat($('st-link'), '探测中', 'warn'); break;
    default:             setStat($('st-link'), '—', '');
  }

  setStat($('st-account'), st.account || '—');
  setStat($('st-ident'), st.identification || '—');

  // 在线主机（只对客户端模式有意义：服务端模式不选目标主机）
  const box = $('hosts-box');
  const list = $('hosts-list');
  const hasHosts = !!(st.hosts && st.hosts.length);
  // 存给 updateModeVisibility 用，让模式切换即时生效
  box.dataset.hasHosts = hasHosts ? '1' : '';
  box.hidden = !isClientMode || !hasHosts;

  if (hasHosts) {
    list.innerHTML = '';
    st.hosts.forEach((h) => {
      const li = document.createElement('li');
      const name = document.createElement('span');
      name.textContent = h.name || '(未命名)';
      const id = document.createElement('span');
      id.className = 'id';
      id.textContent = h.id;
      id.title = '点击填入「目标识别码」';
      id.onclick = () => {
        // id 必须与 FIELDS 推导出的 DOM id 一致（targetHostId → f-target-host-id），
        // 否则点选能填进框里、保存却读不到，识别码永远不会被写进配置
        $('f-target-host-id').value = h.id;
        $('f-target-host-name').value = h.name || '';
        showMsg(`已选择目标：${h.name}`, false);
      };
      li.append(name, id);
      list.appendChild(li);
    });
  }

  // 错误
  const errBox = $('status-error');
  if (st.error) {
    errBox.hidden = false;
    errBox.textContent = st.apiReachable
      ? `${st.error}`
      : `无法连接本地接口：${st.error}\n（若皎月连未运行属正常，保活会自动拉起）`;
  } else {
    errBox.hidden = true;
  }


}

// ---------- 数据加载 ----------
async function loadConfig() {
  try {
    const cfg = await invoke('get_config');
    fillForm(cfg);
  } catch (e) {
    showMsg('读取配置失败：' + e, true);
  }
}

async function loadStatus() {
  try {
    const st = await invoke('get_status');
    renderStatus(st);
  } catch (e) {
    showMsg('获取状态失败：' + e, true);
  }
}

async function loadMisc() {
  try {
    const info = await invoke('app_info');
    $('app-version').textContent = 'v' + info.version;
    document.title = `${info.name} v${info.version}`;
    // 把实际使用的配置文件路径显示出来 —— 便于发现"改错了文件"
    if (info.configPath) {
      const el = $('config-path-inline');
      if (el) el.textContent = info.configPath;
      const el2 = $('config-path-footer');
      if (el2) el2.textContent = info.configPath;
    }
  } catch (_) {}

  try {
    $('f-autostart').checked = await invoke('autostart_status');
  } catch (_) {}

  try {
    const svc = await invoke('service_status');
    setBadge($('badge-service'),
      svc.running ? '运行中' : (svc.installed ? '已安装未运行' : '未安装'),
      svc.running ? 'ok' : '');
  } catch (_) {}
}

// ---------- 事件绑定 ----------
$('btn-refresh').onclick = () => loadStatus();

$('btn-save').onclick = async () => {
  try {
    const view = readForm();
    // 开机自启是注册表操作，不属于 config.json，必须在这里单独同步。
    // 注意要**先**取值：紧接着的 loadConfig() 会把复选框重置为 false，
    // 而保存流程又不刷新它，表现出来就是"一保存就自动取消勾选"。
    const wantAutostart = $('f-autostart')?.checked ?? false;

    let msg = await invoke('save_config', { view });

    try {
      const r = await invoke('autostart_set', { enable: wantAutostart });
      if (r) msg += ' · ' + r;
    } catch (e) {
      msg += ' · 开机自启设置失败：' + errText(e);
    }

    await loadConfig();

    // 用注册表的真实状态回填，保证界面与实际一致
    try {
      $('f-autostart').checked = await invoke('autostart_status');
    } catch (_) { /* 读不到就保持原状 */ }

    await refreshConfigCheck();   // 保存后重新校验
    showMsg(msg, false);
  } catch (e) {
    showMsg('保存失败：' + errText(e), true);
  }
};

// 保活默认随程序自动开启（配置齐全时），界面不提供启停按钮。
// 需要临时停止：托盘菜单 →「退出」，或删除守护进程。
// ---------- 文件选择 ----------
/** 从完整路径里取出文件名（不含扩展名）*/
function stemOf(p) {
  const base = p.split(/[\\/]/).pop() || '';
  return base.replace(/\.exe$/i, '');
}
/** 从完整路径里取出所在目录 */
function dirOf(p) {
  const i = Math.max(p.lastIndexOf('\\'), p.lastIndexOf('/'));
  return i > 0 ? p.slice(0, i) : '';
}

// 选择 natpierce.exe
$('btn-browse-exe').onclick = async () => {
  try {
    const picked = await pickPath({
      filters: [{ name: '可执行文件', extensions: ['exe'] }],
    });
    if (!picked) return;

    $('f-exe-path').value = picked;

    // 自动推导：进程名 = 文件名去 .exe；工作目录 = 所在目录
    const stem = stemOf(picked);
    if (stem) $('f-process-name').value = stem;
    const dir = dirOf(picked);
    if (dir && !$('f-working-dir').value.trim()) $('f-working-dir').value = dir;

    // 顺带提示文件名是否像皎月连
    if (!/natpierce/i.test(stem)) {
      showMsg(`提示：文件名是「${stem}」，皎月连通常叫 natpierce.exe，请确认选对了`, true);
    } else {
      showMsg(`已选择：${stem}，记得点「保存配置」`, false);
    }
  } catch (e) {
    showMsg(errText(e), true);
  }
};

// 选择工作目录
$('btn-browse-dir').onclick = async () => {
  try {
    const picked = await pickPath({ directory: true });
    if (!picked) return;
    $('f-working-dir').value = picked;
    showMsg('已选择工作目录，记得点「保存配置」', false);
  } catch (e) {
    showMsg(errText(e), true);
  }
};

// ---------- 在资源管理器里打开目录 ----------
/**
 * which = 'log' 打开日志目录，其它值打开配置文件所在目录
 */
async function openDir(which) {
  try {
    await invoke('open_dir', { which });
  } catch (e) {
    showMsg(errText(e), true);
  }
}

// 三个入口都绑上；用可选链，避免某个按钮缺失时整段脚本中断
$('btn-open-config')?.addEventListener('click', () => openDir('config'));
$('btn-open-log')?.addEventListener('click', () => openDir('log'));
$('btn-open-config-top')?.addEventListener('click', () => openDir('config'));

// ---------- 运行日志 ----------
let lastLogTotal = -1;

/**
 * 拉取守护进程日志尾部并渲染到界面上。
 * @param {boolean} force 行数没变时也强制重渲染（手动刷新用）
 */
async function loadLogs(force) {
  try {
    const data = await invoke('read_logs', { lines: 200 });
    const box = $('log-view');
    if (!box) return;

    const pathEl = $('log-path');
    if (pathEl) pathEl.textContent = data.path || '—';

    if (!data.exists) {
      box.replaceChildren();
      const d = document.createElement('div');
      d.className = 'log-line lv-WARN';
      d.textContent = '还没有日志文件 —— 启动保活后，这里会显示完整的启动过程。';
      box.appendChild(d);
      lastLogTotal = -1;
      return;
    }

    // 行数没变就不重渲染：省开销，也不会打断用户正在看的滚动位置
    if (!force && data.total === lastLogTotal) return;
    lastLogTotal = data.total;

    const frag = document.createDocumentFragment();
    for (const line of data.lines) {
      const div = document.createElement('div');
      let lv = 'INFO';
      if (line.includes(' ERROR ')) lv = 'ERROR';
      else if (line.includes(' WARN ')) lv = 'WARN';
      div.className = 'log-line lv-' + lv;
      div.textContent = line; // textContent 自动转义，日志内容不会破坏页面结构
      frag.appendChild(div);
    }
    box.replaceChildren(frag);

    if ($('log-autoscroll')?.checked) box.scrollTop = box.scrollHeight;
  } catch (e) {
    // 日志读取失败不能影响主界面
    console.error('读取日志失败', e);
  }
}

$('btn-log-refresh')?.addEventListener('click', () => loadLogs(true));

document.querySelectorAll('input[name="mode"]').forEach((r) => {
  r.onchange = () => {
    updateModeVisibility();
    refreshConfigCheck();   // 换模式后必填项不同，校验结果也要跟着变
  };
});

// 注：托盘菜单已精简为「打开设置…/退出」，不再发 refresh 事件。
// 状态本来就由后端每 3 秒主动推送，无需手动刷新入口。

// 后端主动推送的状态（取代轮询，操作完成即刷新）
listenEvent('status-changed', (ev) => {
  try {
    renderStatus(ev.payload);
    setProbing(false);
  } catch (e) {
    console.error('渲染推送状态失败', e);
  }
  // 日志跟着状态一起刷：启动/重连过程能立刻看到
  loadLogs(false);
});

// 探测阶段：probing → 显示"检测中…"
listenEvent('probe-state', (ev) => {
  setProbing(ev.payload === 'probing');
});

// 兜底：任何未捕获异常都显示出来，避免"界面无反应却不知为何"
window.addEventListener('error', (ev) => {
  showMsg('界面错误：' + errText(ev.error || ev.message), true);
});
window.addEventListener('unhandledrejection', (ev) => {
  showMsg('异步错误：' + errText(ev.reason), true);
});

// ---------- 启动 ----------
(async function init() {
  if (!tauriApi()) {
    $('status-error').hidden = false;
    $('status-error').textContent =
      '未检测到 Tauri API（window.__TAURI__）。\n' +
      '请确认 tauri.conf.json 中已设置 app.withGlobalTauri = true 后重新编译。';
    return;
  }

  // 把窗口背景设成与界面一致的深色，
  // 避免 WebView2 默认纯黑背景在加载前/渲染异常时显示成"黑屏窗口"
  try {
    const win = tauriApi().window.getCurrentWindow();
    if (win.setBackgroundColor) {
      // 与 CSS 的 --bg 一致 (0xRRGGBB)
      await win.setBackgroundColor({ red: 20, green: 22, blue: 28, alpha: 255 });
    }
  } catch (_) { /* 旧版本不支持则忽略 */ }

  await loadConfig();
  await loadMisc();
  await loadStatus();
  await refreshConfigCheck();
  await loadLogs(true);

  // 状态由后端每 3 秒主动推送（status-changed 事件），这里不再轮询。
  // 这样操作完成时能立即反映，而不用等下一个轮询周期。
})();
