/* ===== Tomato Novel Downloader – WebUI ===== */

let loginPromise = null;
let isDockerBuild = false;

function fetchWithCreds(url, opts) {
  return fetch(url, { credentials: 'same-origin', ...(opts || {}) });
}

// ── Theme ──────────────────────────────────────────────────────────

const THEME_KEY = 'tnd.theme';

function getStoredTheme() {
  try { return localStorage.getItem(THEME_KEY); } catch { return null; }
}

function applyTheme(theme) {
  if (theme === 'light' || theme === 'dark') {
    document.documentElement.setAttribute('data-theme', theme);
  } else {
    document.documentElement.removeAttribute('data-theme');
  }
  updateThemeButton(theme);
}

function updateThemeButton(theme) {
  const icon = document.getElementById('themeIcon');
  const label = document.getElementById('themeLabel');
  if (!icon) return;

  const isDark = theme === 'dark' ||
    (!theme && window.matchMedia('(prefers-color-scheme: dark)').matches);

  if (isDark) {
    icon.innerHTML = '<circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/>';
    if (label) label.textContent = '亮色模式';
  } else {
    icon.innerHTML = '<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>';
    if (label) label.textContent = '暗色模式';
  }
}

function toggleTheme() {
  const current = document.documentElement.getAttribute('data-theme');
  let next;
  if (current === 'dark') {
    next = 'light';
  } else if (current === 'light') {
    next = 'dark';
  } else {
    // auto → opposite of system
    next = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'light' : 'dark';
  }
  try { localStorage.setItem(THEME_KEY, next); } catch {}
  applyTheme(next);
}

// Apply stored theme immediately
(function() {
  const stored = getStoredTheme();
  if (stored) applyTheme(stored);
})();

// ── Auth ───────────────────────────────────────────────────────────

function showLogin(show) {
  const modal = document.getElementById('loginModal');
  if (!modal) return;
  modal.classList.toggle('hidden', !show);
  document.body.style.overflow = show ? 'hidden' : '';
  if (show) {
    const inp = document.getElementById('loginPassword');
    if (inp) inp.focus();
  }
}

async function requireLogin() {
  if (loginPromise) return loginPromise;

  showLogin(true);
  const msg = document.getElementById('loginMsg');
  if (msg) msg.textContent = '';

  loginPromise = new Promise((resolve, reject) => {
    const form = document.getElementById('loginForm');
    if (!form) { reject(new Error('login form missing')); return; }

    const handler = async (e) => {
      e.preventDefault();
      const pw = (document.getElementById('loginPassword')?.value || '').toString();
      try {
        const res = await fetchWithCreds('/api/login', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ password: pw })
        });
        if (!res.ok) { if (msg) msg.textContent = '密码错误'; return; }
        showLogin(false);
        form.removeEventListener('submit', handler);
        resolve(true);
      } catch (err) {
        if (msg) msg.textContent = String(err || 'login failed');
      }
    };
    form.addEventListener('submit', handler);
  }).finally(() => { loginPromise = null; });

  return loginPromise;
}

// ── HTTP Helper ────────────────────────────────────────────────────

async function j(url, opts) {
  const res = await fetchWithCreds(url, opts);
  if (res.status === 401) {
    await requireLogin();
    const res2 = await fetchWithCreds(url, opts);
    if (!res2.ok) {
      const text = await res2.text().catch(() => '');
      throw new Error(`${res2.status} ${res2.statusText}${text ? `: ${text}` : ''}`);
    }
    const ct2 = res2.headers.get('content-type') || '';
    return ct2.includes('application/json') ? res2.json() : res2.text();
  }
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`${res.status} ${res.statusText}${text ? `: ${text}` : ''}`);
  }
  const ct = res.headers.get('content-type') || '';
  return ct.includes('application/json') ? res.json() : res.text();
}

// ── Utilities ──────────────────────────────────────────────────────

function esc(s) {
  return (s ?? '').toString().replace(/[&<>"']/g, c =>
    ({ '&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;' }[c]));
}

function fmtBytes(n) {
  const x = Number(n || 0);
  if (!isFinite(x) || x <= 0) return '0 B';
  const k = 1024;
  const sizes = ['B','KB','MB','GB','TB'];
  const i = Math.floor(Math.log(x) / Math.log(k));
  return (x / Math.pow(k, i)).toFixed(i === 0 ? 0 : 1) + ' ' + sizes[i];
}

function fmtTime(ms) {
  const x = Number(ms || 0);
  if (!isFinite(x) || x <= 0) return '';
  return new Date(x).toLocaleString();
}

function encodePathSegments(path) {
  return (path || '').toString().split('/').map(seg => encodeURIComponent(seg)).join('/');
}

function parseBookId(input) {
  const trimmed = (input ?? '').toString().trim();
  if (!trimmed) return '';
  if (/^[0-9]+$/.test(trimmed)) return trimmed;

  const urlMatch = trimmed.match(/https?:\/\/\S+/i);
  const target = urlMatch ? urlMatch[0] : trimmed;

  const qs = target.match(/(?:^|[?&#])(?:book_id|bookId)=([0-9]+)/i);
  if (qs && qs[1]) return qs[1];

  const page = target.match(/\/page\/([0-9]+)/i);
  if (page && page[1]) return page[1];

  return '';
}

// ── App Update ─────────────────────────────────────────────────────

const DISMISS_KEY = 'tnd.dismissed_release_tag';

function getDismissedTag() {
  try { return (localStorage.getItem(DISMISS_KEY) || '').toString(); } catch { return ''; }
}
function setDismissedTag(tag) {
  try { localStorage.setItem(DISMISS_KEY, (tag || '').toString()); } catch {}
}

function showAppUpdateBanner(show) {
  const el = document.getElementById('appUpdateBanner');
  if (el) el.classList.toggle('hidden', !show);
}

function applyDockerUpdateUi() {
  if (!isDockerBuild) return;
  const hint = document.getElementById('appUpdateHint');
  if (hint) hint.textContent = 'Docker 构建已禁用程序自更新，请通过重新拉取镜像升级。';
  showAppUpdateBanner(false);
  const btn = document.getElementById('appUpdateCheck');
  if (btn) btn.disabled = true;
  const selfBtn = document.getElementById('appSelfUpdate');
  if (selfBtn) selfBtn.disabled = true;
  const dismissBtn = document.getElementById('appUpdateDismiss');
  if (dismissBtn) dismissBtn.disabled = true;
}

async function refreshAppUpdate(manual) {
  const hint = document.getElementById('appUpdateHint');
  const latestEl = document.getElementById('appUpdateLatest');
  const bodyEl = document.getElementById('appUpdateBody');
  const linkEl = document.getElementById('appUpdateLink');

  if (isDockerBuild) {
    applyDockerUpdateUi();
    if (latestEl) latestEl.textContent = '';
    if (bodyEl) bodyEl.textContent = 'Docker 构建已禁用程序自更新，请通过重新拉取镜像升级。';
    if (linkEl) linkEl.style.pointerEvents = 'none';
    return { latestTag: '', hasUpdate: false, dockerBuild: true };
  }

  if (hint) hint.textContent = manual ? '检查中…' : '';

  const data = await j('/api/app_update');
  const latestTag = (data.latest_tag || '').toString();
  const latestBody = (data.latest_body || '').toString();
  const latestUrl = (data.latest_url || '').toString();
  const hasUpdate = !!data.has_update;

  if (latestEl) latestEl.textContent = latestTag || '';
  if (bodyEl) bodyEl.textContent = latestBody || '';
  if (linkEl) {
    linkEl.href = latestUrl || '#';
    linkEl.style.pointerEvents = latestUrl ? '' : 'none';
  }

  const dismissed = getDismissedTag();
  const shouldShow = hasUpdate && latestTag && dismissed !== latestTag;

  if (shouldShow) {
    showAppUpdateBanner(true);
    if (hint) hint.textContent = '发现新版本';
  } else {
    showAppUpdateBanner(false);
    if (manual) {
      if (!hasUpdate) {
        if (hint) hint.textContent = '已是最新版本';
      } else if (dismissed === latestTag) {
        if (hint) hint.textContent = '已忽略该版本提醒';
      }
    }
  }
  return { latestTag, hasUpdate };
}

// ── Status ─────────────────────────────────────────────────────────

let libraryPath = '';
let pendingBookNameJobId = null;
let pendingBookNameOptions = [];

async function refreshStatus() {
  const data = await j('/api/status');
  document.getElementById('version').textContent = data.version || '';
  document.getElementById('prewarm').textContent = data.prewarm_in_progress ? 'warming' : 'ready';
  document.getElementById('saveDir').textContent = data.save_dir || '';
  document.getElementById('bind').textContent = data.bind_addr || '';
  document.getElementById('locked').textContent = data.locked ? 'locked' : 'unlocked';
  isDockerBuild = !!data.docker_build;
  applyDockerUpdateUi();
}

// ── Config ─────────────────────────────────────────────────────────

async function refreshConfig() {
  const data = await j('/api/config');
  const nf = document.getElementById('cfgNovelFormat');
  const bf = document.getElementById('cfgBulkFiles');
  const ea = document.getElementById('cfgEnableAudiobook');
  const af = document.getElementById('cfgAudiobookFormat');
  if (nf) nf.value = (data.novel_format || 'txt').toString();
  if (bf) bf.checked = !!data.bulk_files;
  if (ea) ea.checked = !!data.enable_audiobook;
  if (af) af.value = (data.audiobook_format || 'mp3').toString();
}

async function saveConfig() {
  const nf = document.getElementById('cfgNovelFormat')?.value;
  const bf = !!document.getElementById('cfgBulkFiles')?.checked;
  const ea = !!document.getElementById('cfgEnableAudiobook')?.checked;
  const af = document.getElementById('cfgAudiobookFormat')?.value;

  await j('/api/config', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ novel_format: nf, bulk_files: bf, enable_audiobook: ea, audiobook_format: af })
  });
}

async function refreshRawConfig() {
  const data = await j('/api/config/raw');
  const ta = document.getElementById('cfgRaw');
  const msg = document.getElementById('cfgRawMsg');
  if (ta) ta.value = (data.yaml || '').toString();
  if (msg) msg.textContent = data.generated ? '已生成默认配置（未找到配置文件）' : '';
}

async function saveRawConfig() {
  const ta = document.getElementById('cfgRaw');
  const yaml = (ta?.value || '').toString();
  await j('/api/config/raw', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ yaml })
  });
}

// ── Full Config ────────────────────────────────────────────────────

let currentFullConfig = null;

const FULL_CONFIG_SCHEMA = [
  {
    title: '基础与格式',
    fields: [
      { key: 'save_path', label: '保存路径', type: 'text' },
      { key: 'novel_format', label: '小说格式', type: 'select', options: [
        { value: 'txt', label: 'txt' }, { value: 'epub', label: 'epub' }
      ] },
      { key: 'first_line_indent_em', label: '首行缩进(em)', type: 'number', parse: 'float', step: '0.1', min: '0' },
      { key: 'bulk_files', label: '散装文件保存', type: 'bool' },
      { key: 'auto_clear_dump', label: '自动清理缓存', type: 'bool' },
      { key: 'auto_open_downloaded_files', label: '下载完成后自动打开', type: 'bool' },
      { key: 'allow_overwrite_files', label: '允许覆盖已存在文件', type: 'bool' },
      { key: 'preferred_book_name_field', label: '优先书名字段', type: 'select', options: [
        { value: 'book_name', label: '默认书名' },
        { value: 'original_book_name', label: '原始书名' },
        { value: 'book_short_name', label: '短书名' },
        { value: 'ask_after_download', label: '下载完后选择' }
      ] },
      { key: 'old_cli', label: '旧版 CLI UI', type: 'bool' },
    ]
  },
  {
    title: '网络与调度',
    fields: [
      { key: 'max_workers', label: '最大线程数', type: 'number', parse: 'int', min: '1' },
      { key: 'request_timeout', label: '请求超时(s)', type: 'number', parse: 'int', min: '1' },
      { key: 'max_retries', label: '最大重试次数', type: 'number', parse: 'int', min: '0' },
      { key: 'min_connect_timeout', label: '最小连接超时(s)', type: 'number', parse: 'float', step: '0.1', min: '0' },
      { key: 'min_wait_time', label: '最小等待时间(ms)', type: 'number', parse: 'int', min: '0' },
      { key: 'max_wait_time', label: '最大等待时间(ms)', type: 'number', parse: 'int', min: '0' },
    ]
  },
  {
    title: 'API',
    fields: [
      { key: 'use_official_api', label: '使用官方 API', type: 'bool' },
      { key: 'api_endpoints', label: 'API 列表', type: 'list', placeholder: '每行一条或用逗号分隔' },
    ]
  },
  {
    title: '段评',
    fields: [
      { key: 'enable_segment_comments', label: '启用段评', type: 'bool' },
      { key: 'segment_comments_top_n', label: '每段评论数上限', type: 'number', parse: 'int', min: '1' },
      { key: 'segment_comments_workers', label: '段评并发线程数', type: 'number', parse: 'int', min: '1' },
    ]
  },
  {
    title: '媒体下载',
    fields: [
      { key: 'download_comment_images', label: '下载评论图片', type: 'bool' },
      { key: 'download_comment_avatars', label: '下载评论头像', type: 'bool' },
      { key: 'media_download_workers', label: '媒体下载线程数', type: 'number', parse: 'int', min: '1' },
      { key: 'blocked_media_domains', label: '阻止的图片域名', type: 'list', placeholder: '每行一个域名' },
      { key: 'force_convert_images_to_jpeg', label: '强制转成 JPEG', type: 'bool' },
      { key: 'jpeg_retry_convert', label: '失败重试再转 JPEG', type: 'bool' },
      { key: 'jpeg_quality', label: 'JPEG 质量(0-100)', type: 'number', parse: 'int', min: '0', max: '100' },
      { key: 'convert_heic_to_jpeg', label: 'HEIC 转 JPEG', type: 'bool' },
      { key: 'keep_heic_original', label: '保留 HEIC 原图', type: 'bool' },
      { key: 'media_limit_per_chapter', label: '单章节媒体上限', type: 'number', parse: 'int', min: '0' },
      { key: 'media_max_dimension_px', label: '媒体最大尺寸(px)', type: 'number', parse: 'int', min: '0' },
    ]
  },
  {
    title: '有声书',
    fields: [
      { key: 'enable_audiobook', label: '启用有声书', type: 'bool' },
      { key: 'audiobook_voice', label: '发音人', type: 'voice' },
      { key: 'audiobook_tts_provider', label: 'TTS 服务类型', type: 'select', options: [
        { value: 'edge', label: 'edge' }, { value: 'third_party', label: 'third_party' }
      ] },
      { key: 'audiobook_tts_api_url', label: '第三方 TTS API 地址', type: 'text' },
      { key: 'audiobook_tts_api_token', label: '第三方 TTS Token', type: 'text' },
      { key: 'audiobook_tts_model', label: '第三方 TTS 模型', type: 'text' },
      { key: 'audiobook_rate', label: '语速调整', type: 'text' },
      { key: 'audiobook_volume', label: '音量调整', type: 'text' },
      { key: 'audiobook_pitch', label: '音调调整', type: 'text' },
      { key: 'audiobook_format', label: '输出格式', type: 'select', options: [
        { value: 'mp3', label: 'mp3' }, { value: 'wav', label: 'wav' }
      ] },
      { key: 'audiobook_concurrency', label: '并发生成章节数', type: 'number', parse: 'int', min: '1' },
    ]
  },
];

const AUDIOBOOK_VOICE_PRESETS = [
  { value: 'zh-CN-XiaoxiaoNeural', label: 'zh-CN-XiaoxiaoNeural (女)' },
  { value: 'zh-CN-XiaoyiNeural', label: 'zh-CN-XiaoyiNeural (女)' },
  { value: 'zh-CN-YunjianNeural', label: 'zh-CN-YunjianNeural (男)' },
  { value: 'zh-CN-YunxiNeural', label: 'zh-CN-YunxiNeural (男)' },
  { value: 'zh-CN-YunxiaNeural', label: 'zh-CN-YunxiaNeural (男)' },
  { value: 'zh-CN-YunyangNeural', label: 'zh-CN-YunyangNeural (男)' },
  { value: 'zh-CN-liaoning-XiaobeiNeural', label: 'zh-CN-liaoning-XiaobeiNeural (女)' },
  { value: 'zh-CN-shaanxi-XiaoniNeural', label: 'zh-CN-shaanxi-XiaoniNeural (女)' },
  { value: 'zh-HK-HiuGaaiNeural', label: 'zh-HK-HiuGaaiNeural (女)' },
  { value: 'zh-HK-HiuMaanNeural', label: 'zh-HK-HiuMaanNeural (女)' },
  { value: 'zh-HK-WanLungNeural', label: 'zh-HK-WanLungNeural (男)' },
  { value: 'zh-TW-HsiaoChenNeural', label: 'zh-TW-HsiaoChenNeural (女)' },
];

function renderFullConfigForm(cfg) {
  const body = document.getElementById('configFullBody');
  if (!body) return;
  body.innerHTML = '';

  for (const section of FULL_CONFIG_SCHEMA) {
    const sec = document.createElement('div');
    sec.className = 'configSection';
    sec.innerHTML = `<h4>${esc(section.title)}</h4>`;
    body.appendChild(sec);

    for (const field of section.fields) {
      const row = document.createElement('div');
      row.className = 'config-field';

      const label = document.createElement('span');
      label.className = 'field-label';
      label.textContent = field.label;
      row.appendChild(label);

      let input;
      if (field.type === 'bool') {
        input = document.createElement('input');
        input.type = 'checkbox';
        input.checked = !!cfg[field.key];
      } else if (field.type === 'voice') {
        input = document.createElement('div');
        input.className = 'voiceRow';
        const select = document.createElement('select');
        const emptyOpt = document.createElement('option');
        emptyOpt.value = '';
        emptyOpt.textContent = '自定义...';
        select.appendChild(emptyOpt);
        for (const opt of AUDIOBOOK_VOICE_PRESETS) {
          const o = document.createElement('option');
          o.value = opt.value;
          o.textContent = opt.label;
          select.appendChild(o);
        }
        const text = document.createElement('input');
        text.type = 'text';
        text.value = (cfg[field.key] ?? '').toString();
        text.placeholder = '输入或选择发音人';
        text.dataset.key = field.key;
        text.dataset.type = 'text';
        text.dataset.voiceInput = '1';

        const current = (cfg[field.key] ?? '').toString();
        const preset = AUDIOBOOK_VOICE_PRESETS.find(p => p.value === current);
        select.value = preset ? preset.value : '';

        select.addEventListener('change', () => { if (select.value) text.value = select.value; });
        input.appendChild(select);
        input.appendChild(text);
      } else if (field.type === 'select') {
        input = document.createElement('select');
        for (const opt of field.options || []) {
          const o = document.createElement('option');
          o.value = opt.value;
          o.textContent = opt.label;
          input.appendChild(o);
        }
        input.value = (cfg[field.key] ?? '').toString();
      } else if (field.type === 'list') {
        input = document.createElement('textarea');
        input.value = Array.isArray(cfg[field.key]) ? cfg[field.key].join('\n') : '';
        input.placeholder = field.placeholder || '';
        input.classList.add('cfgList');
      } else if (field.type === 'number') {
        input = document.createElement('input');
        input.type = 'number';
        if (field.step) input.step = field.step;
        if (field.min) input.min = field.min;
        if (field.max) input.max = field.max;
        input.value = (cfg[field.key] ?? '').toString();
      } else {
        input = document.createElement('input');
        input.type = 'text';
        input.value = (cfg[field.key] ?? '').toString();
        if (field.placeholder) input.placeholder = field.placeholder;
      }

      if (field.type !== 'voice') {
        input.dataset.key = field.key;
        input.dataset.type = field.type;
        if (field.parse) input.dataset.parse = field.parse;
      }

      row.appendChild(input);
      sec.appendChild(row);
    }
  }
}

async function loadFullConfigPanel() {
  const msg = document.getElementById('cfgFullMsg');
  if (msg) msg.textContent = '加载中…';
  try {
    const cfg = await j('/api/config/full');
    currentFullConfig = cfg || {};
    renderFullConfigForm(currentFullConfig);
    if (msg) msg.textContent = '';
  } catch (err) {
    if (msg) msg.textContent = '加载失败';
  }
}

function collectFullConfig() {
  const out = { ...(currentFullConfig || {}) };
  const body = document.getElementById('configFullBody');
  if (!body) return out;
  const inputs = body.querySelectorAll('[data-key]');
  for (const el of inputs) {
    const key = el.dataset.key;
    const type = el.dataset.type;
    if (!key || !type) continue;
    if (type === 'bool') {
      out[key] = !!el.checked;
    } else if (type === 'list') {
      out[key] = (el.value || '').toString().split(/[\n,;]/).map(s => s.trim()).filter(s => s.length > 0);
    } else if (type === 'number') {
      const raw = (el.value || '').toString().trim();
      if (!raw) continue;
      const parse = el.dataset.parse || 'int';
      const val = parse === 'float' ? parseFloat(raw) : parseInt(raw, 10);
      if (!Number.isNaN(val)) out[key] = val;
    } else {
      out[key] = (el.value || '').toString();
    }
  }
  return out;
}

async function saveFullConfig() {
  const cfg = collectFullConfig();
  await j('/api/config/full', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(cfg)
  });
}

// ── Library ────────────────────────────────────────────────────────

async function refreshLibrary() {
  const qs = libraryPath ? `?path=${encodeURIComponent(libraryPath)}` : '';
  const data = await j(`/api/library${qs}`);
  const items = data.items || [];
  libraryPath = (data.path || '').toString();

  const pathLabel = document.getElementById('libPath');
  const backBtn = document.getElementById('libBack');
  if (pathLabel) pathLabel.textContent = libraryPath ? `/${libraryPath}` : '/';
  if (backBtn) backBtn.disabled = !libraryPath;

  const tbody = document.getElementById('libraryBody');
  tbody.innerHTML = '';
  for (const it of items) {
    const tr = document.createElement('tr');
    const kind = it.kind || 'file';
    const rel = it.rel_path || '';
    const name = it.name || rel;
    const encodedRel = encodePathSegments(rel);
    const hrefFile = `/download/${encodedRel}`;
    const hrefZip = `/download-zip/${encodedRel}`;
    const sizeText = kind === 'dir'
      ? `${fmtBytes(it.size)} (${Number(it.file_count || 0)} 文件)`
      : fmtBytes(it.size);
    const timeText = fmtTime(it.modified_ms);

    if (kind === 'dir') {
      tr.innerHTML = `
        <td><button class="openDir sm" data-path="${esc(rel)}">打开</button> ${esc(name)} <span class="badge">文件夹</span></td>
        <td>${esc(sizeText)}</td>
        <td>${esc(timeText)}</td>
        <td><a href="${hrefZip}">打包下载</a></td>
      `;
    } else {
      tr.innerHTML = `
        <td><a href="${hrefFile}">${esc(name)}</a> <span class="badge">${esc(it.ext || '')}</span></td>
        <td>${esc(sizeText)}</td>
        <td>${esc(timeText)}</td>
        <td><a href="${hrefFile}">下载</a></td>
      `;
    }
    tbody.appendChild(tr);
  }
  if (items.length === 0) {
    tbody.innerHTML = '<tr class="empty-row"><td colspan="4">暂无文件，先下载一本书吧</td></tr>';
  }
}

// ── Search ─────────────────────────────────────────────────────────

async function doSearch(q) {
  const out = document.getElementById('searchResults');
  out.innerHTML = '';
  if (!q) return;
  const data = await j(`/api/search?q=${encodeURIComponent(q)}`);
  const items = data.items || [];
  if (items.length === 0) {
    out.innerHTML = '<tr class="empty-row"><td colspan="4">无结果</td></tr>';
    return;
  }
  for (const b of items) {
    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td>${esc(b.title ?? '')}</td>
      <td>${esc(b.author ?? '')}</td>
      <td><code>${esc(b.book_id)}</code></td>
      <td><button data-bookid="${esc(b.book_id)}" class="startDownload sm primary">下载</button></td>
    `;
    out.appendChild(tr);
  }
}

// ── Preview ────────────────────────────────────────────────────────

let currentPreviewBookId = null;
let currentPreviewData = null;

function showPreviewModal(show) {
  const modal = document.getElementById('previewModal');
  if (!modal) return;
  modal.classList.toggle('hidden', !show);
  document.body.style.overflow = show ? 'hidden' : '';
  if (!show) {
    currentPreviewBookId = null;
    currentPreviewData = null;
  }
}

async function openPreview(bookId) {
  currentPreviewBookId = bookId;
  currentPreviewData = null;
  showPreviewModal(true);

  const loading = document.getElementById('previewLoading');
  const data = document.getElementById('previewData');
  const rangeInput = document.getElementById('previewRangeInput');
  const rangeHint = document.getElementById('previewRangeHint');

  if (loading) loading.classList.remove('hidden');
  if (data) data.classList.add('hidden');
  if (rangeInput) rangeInput.value = '';
  if (rangeHint) { rangeHint.textContent = ''; rangeHint.classList.remove('error'); }

  try {
    const preview = await j(`/api/preview/${encodeURIComponent(bookId)}`);
    currentPreviewData = preview;

    if (loading) loading.classList.add('hidden');
    if (data) data.classList.remove('hidden');

    const title = document.getElementById('previewTitle');
    const origTitle = document.getElementById('previewOrigTitle');
    const author = document.getElementById('previewAuthor');
    const stats = document.getElementById('previewStats');
    const desc = document.getElementById('previewDesc');
    const tags = document.getElementById('previewTags');
    const chapters = document.getElementById('previewChapters');
    const cover = document.getElementById('previewCover');

    if (title) title.textContent = preview.book_name || '未知书名';

    if (origTitle) {
      if (preview.original_book_name && preview.original_book_name !== preview.book_name) {
        origTitle.textContent = `原名: ${preview.original_book_name}`;
        origTitle.classList.remove('hidden');
      } else {
        origTitle.classList.add('hidden');
      }
    }

    if (author) author.textContent = preview.author ? `作者: ${preview.author}` : '作者: 未知';

    if (stats) {
      const parts = [];
      if (preview.chapter_count) parts.push(`章节: ${preview.chapter_count}`);
      if (preview.finished !== null && preview.finished !== undefined) {
        parts.push(`状态: ${preview.finished ? '完结' : '连载'}`);
      }
      if (preview.word_count) {
        const words = Number(preview.word_count);
        parts.push(`字数: ${words >= 10000 ? (words / 10000).toFixed(1) + '万' : words}字`);
      }
      if (preview.score != null) parts.push(`评分: ${preview.score.toFixed(1)}`);
      if (preview.read_count_text || preview.read_count) {
        parts.push(`阅读: ${preview.read_count_text || preview.read_count}`);
      }
      stats.innerHTML = '';
      parts.forEach(p => {
        const span = document.createElement('span');
        span.textContent = p;
        stats.appendChild(span);
      });
    }

    if (desc) desc.textContent = preview.description || '暂无简介';

    if (tags) {
      if (preview.tags && preview.tags.length > 0) {
        tags.innerHTML = '';
        preview.tags.forEach(t => {
          const badge = document.createElement('span');
          badge.className = 'badge';
          badge.textContent = t;
          tags.appendChild(badge);
        });
        tags.classList.remove('hidden');
      } else {
        tags.classList.add('hidden');
      }
    }

    if (chapters) {
      const chapterInfo = [];
      if (preview.chapter_count) chapterInfo.push(`总章节数: ${preview.chapter_count}`);
      if (preview.first_chapter_title) chapterInfo.push(`首章: ${preview.first_chapter_title}`);
      if (preview.last_chapter_title) chapterInfo.push(`末章: ${preview.last_chapter_title}`);
      if (preview.category) chapterInfo.push(`分类: ${preview.category}`);
      chapters.innerHTML = '';
      chapterInfo.forEach(info => {
        const div = document.createElement('div');
        div.textContent = info;
        chapters.appendChild(div);
      });
    }

    if (cover) {
      const coverUrl = preview.detail_cover_url || preview.cover_url;
      if (coverUrl && (coverUrl.startsWith('http://') || coverUrl.startsWith('https://'))) {
        cover.src = coverUrl;
        cover.classList.remove('hidden');
      } else {
        cover.classList.add('hidden');
      }
    }

    if (rangeHint && preview.chapter_count) {
      rangeHint.textContent = `例如: 1-10 下载第1到第10章，1-${preview.chapter_count} 下载全部`;
    }
  } catch (err) {
    if (loading) loading.textContent = `加载失败: ${err}`;
    console.error('Preview load error:', err);
  }
}

async function confirmPreview() {
  if (!currentPreviewBookId || !currentPreviewData) { showPreviewModal(false); return; }

  const bookId = currentPreviewBookId;
  const rangeInput = document.getElementById('previewRangeInput');
  const rangeHint = document.getElementById('previewRangeHint');
  const rangeText = rangeInput ? rangeInput.value.trim() : '';

  let rangeStart = null;
  let rangeEnd = null;

  if (rangeText) {
    const total = currentPreviewData.chapter_count || 0;
    if (total === 0) {
      if (rangeHint) { rangeHint.textContent = '章节数未知，无法使用范围下载'; rangeHint.classList.add('error'); }
      return;
    }
    const parts = rangeText.split('-').map(p => p.trim());
    if (parts.length === 2) {
      const start = parts[0] === '' ? 1 : parseInt(parts[0], 10);
      const end = parts[1] === '' ? total : parseInt(parts[1], 10);
      if (isNaN(start) || isNaN(end) || start < 1 || end < 1 || start > end || end > total) {
        if (rangeHint) { rangeHint.textContent = `范围无效 (1-${total})`; rangeHint.classList.add('error'); }
        return;
      }
      rangeStart = start;
      rangeEnd = end;
    } else {
      if (rangeHint) { rangeHint.textContent = '格式应为 start-end，例如 1-10'; rangeHint.classList.add('error'); }
      return;
    }
  }

  if (rangeHint) rangeHint.classList.remove('error');
  showPreviewModal(false);

  try {
    const payload = { book_id: bookId };
    if (rangeStart !== null && rangeEnd !== null) {
      payload.range_start = rangeStart;
      payload.range_end = rangeEnd;
    }
    await j('/api/jobs', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload)
    });
    await refreshJobs();
    window.location.hash = '#jobs';
    const hint = document.getElementById('searchHint');
    if (hint) {
      hint.textContent = rangeStart && rangeEnd
        ? `已创建下载任务：${bookId} (章节 ${rangeStart}-${rangeEnd})`
        : `已创建下载任务：${bookId}`;
    }
  } catch (err) {
    alert(`创建任务失败: ${err}`);
  }
}

async function startDownload(bookId) {
  await openPreview(bookId);
  return null;
}

async function startDownloadDirect(bookId) {
  const job = await j('/api/jobs', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ book_id: bookId })
  });
  await refreshJobs();
  return job;
}

// ── Jobs ───────────────────────────────────────────────────────────

async function refreshJobs() {
  const data = await j('/api/jobs');
  const tbody = document.getElementById('jobsBody');
  tbody.innerHTML = '';
  for (const it of data.items || []) {
    const tr = document.createElement('tr');
    const saved = it.progress ? it.progress.saved_chapters : 0;
    const total = it.progress ? it.progress.chapter_total : 0;
    const pct = total > 0 ? Math.min(100, Math.round((saved / total) * 100)) : 0;
    const progressText = it.progress ? `${saved}/${total}` : '';
    const title = it.title || it.book_id || '';

    // Determine effective visual state
    let vState = (it.state || '').toLowerCase();
    if (vState === 'done' && total > 0 && saved < total) vState = 'partial';

    // Row class & CSS custom property for progress gradient
    tr.className = 'job-row state-' + vState;
    if (vState === 'running' || vState === 'queued') {
      tr.style.setProperty('--progress', pct + '%');
    }

    // State badge
    let stateHtml;
    switch (vState) {
      case 'running': stateHtml = `<span class="badge info">${pct}%</span>`; break;
      case 'queued':  stateHtml = '<span class="badge">排队中</span>'; break;
      case 'done':    stateHtml = '<span class="badge success">完成</span>'; break;
      case 'failed':  stateHtml = '<span class="badge danger">失败</span>'; break;
      case 'partial': stateHtml = '<span class="badge warning">部分失败</span>'; break;
      case 'canceled':stateHtml = '<span class="badge">已取消</span>'; break;
      default:        stateHtml = esc(it.state || '');
    }

    // Action button
    let btnHtml;
    switch (vState) {
      case 'done':
        btnHtml = `<button data-jobid="${esc(it.id)}" data-title="${esc(title)}" class="goLibrary sm success">完成</button>`;
        break;
      case 'failed':
      case 'partial':
        btnHtml = `<button data-jobid="${esc(it.id)}" data-bookid="${esc(it.book_id)}" class="retryJob sm warning">重试</button>`;
        break;
      case 'canceled':
        btnHtml = `<button data-jobid="${esc(it.id)}" data-bookid="${esc(it.book_id)}" class="retryJob sm">重试</button>`;
        break;
      default: // running / queued
        btnHtml = `<button data-jobid="${esc(it.id)}" class="cancelJob sm">取消</button>`;
    }

    tr.innerHTML = `
      <td><span class="badge">${esc(it.id)}</span></td>
      <td>${esc(title)}</td>
      <td>${stateHtml}</td>
      <td>${esc(progressText)}</td>
      <td>${btnHtml}</td>
    `;
    tbody.appendChild(tr);
  }
  if ((data.items || []).length === 0) {
    tbody.innerHTML = '<tr class="empty-row"><td colspan="5">暂无任务</td></tr>';
  }

  const pending = (data.items || []).find(it => (it.book_name_options || []).length > 0);
  if (pending && !isBookNameModalOpen()) showBookNameModal(pending);
}

// ── Updates ────────────────────────────────────────────────────────

async function refreshUpdates() {
  const hint = document.getElementById('updatesHint');
  const tbody = document.getElementById('updatesBody');
  if (!tbody) return;

  if (hint) hint.textContent = '扫描中…';
  tbody.innerHTML = '<tr class="empty-row"><td colspan="7">加载中…</td></tr>';

  const data = await j('/api/updates');
  const updates = data.updates || [];
  const noUpdates = data.no_updates || [];
  const total = updates.length + noUpdates.length;

  if (hint) hint.textContent = `可更新 ${updates.length} 本 / 无更新 ${noUpdates.length} 本 / 总计 ${total} 本`;

  tbody.innerHTML = '';
  for (const it of updates) {
    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td>${esc(it.book_name || '')}</td>
      <td><code>${esc(it.book_id || '')}</code></td>
      <td>${esc(Number(it.local_total || 0))}</td>
      <td>${esc(Number(it.remote_total || 0))}</td>
      <td>${esc(Number(it.new_count || 0))}</td>
      <td>${esc(Number(it.local_failed || 0))}</td>
      <td><button data-bookid="${esc(it.book_id || '')}" class="startDownload sm primary">更新</button></td>
    `;
    tbody.appendChild(tr);
  }
  if (updates.length === 0) {
    tbody.innerHTML = '<tr class="empty-row"><td colspan="7">暂无可更新的小说</td></tr>';
  }
}

async function cancelJob(id) {
  await j(`/api/jobs/${encodeURIComponent(id)}/cancel`, { method: 'POST' });
  await refreshJobs();
}

async function clearJob(id) {
  await j(`/api/jobs/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

// ── Book Name Modal ────────────────────────────────────────────────

function isBookNameModalOpen() {
  const modal = document.getElementById('bookNameModal');
  return modal && !modal.classList.contains('hidden');
}

function showBookNameModal(job) {
  pendingBookNameJobId = job.id;
  pendingBookNameOptions = job.book_name_options || [];
  const modal = document.getElementById('bookNameModal');
  const hint = document.getElementById('bookNameJobHint');
  const options = document.getElementById('bookNameOptions');
  if (!modal || !options) return;

  if (hint) {
    const title = job.title || job.book_id || '';
    hint.textContent = title ? `《${title}》` : '';
  }

  options.innerHTML = '';
  pendingBookNameOptions.forEach((opt, idx) => {
    const id = `bookNameOpt_${idx}`;
    const row = document.createElement('label');
    row.className = 'row';
    row.innerHTML = `
      <input type="radio" name="bookNameOpt" id="${id}" value="${esc(opt.value)}" ${idx === 0 ? 'checked' : ''} />
      <span>${esc(opt.label)}: ${esc(opt.value)}</span>
    `;
    options.appendChild(row);
  });
  modal.classList.remove('hidden');
}

async function submitBookNameChoice(value) {
  if (!pendingBookNameJobId) return;
  await j(`/api/jobs/${encodeURIComponent(pendingBookNameJobId)}/book_name`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ value })
  });
  pendingBookNameJobId = null;
  pendingBookNameOptions = [];
  const modal = document.getElementById('bookNameModal');
  if (modal) modal.classList.add('hidden');
  await refreshJobs();
}

// ── Wire ───────────────────────────────────────────────────────────

function wire() {
  // -- Navigation --
  const navLinks = document.querySelectorAll('.nav a');
  const sections = document.querySelectorAll('.section');

  function switchSection(hash) {
    if (!hash) hash = '#status';
    navLinks.forEach(link => {
      link.classList.toggle('active', link.getAttribute('href') === hash);
    });
    sections.forEach(sec => {
      sec.classList.toggle('active', '#' + sec.id === hash);
    });
  }

  window.addEventListener('hashchange', () => switchSection(window.location.hash));
  switchSection(window.location.hash);

  // -- Theme Toggle --
  const themeBtn = document.getElementById('themeToggle');
  if (themeBtn) themeBtn.addEventListener('click', toggleTheme);
  updateThemeButton(getStoredTheme());

  // -- Config Tabs --
  const configTabs = document.querySelectorAll('.config-tab');
  const configPanels = {
    quick: document.getElementById('configPanelQuick'),
    full: document.getElementById('configPanelFull'),
    yaml: document.getElementById('configPanelYaml'),
  };
  let fullConfigLoaded = false;

  configTabs.forEach(tab => {
    tab.addEventListener('click', async () => {
      const target = tab.dataset.tab;
      configTabs.forEach(t => t.classList.toggle('active', t === tab));
      Object.entries(configPanels).forEach(([k, panel]) => {
        if (panel) panel.classList.toggle('active', k === target);
      });

      // Lazy-load full config on first switch
      if (target === 'full' && !fullConfigLoaded) {
        fullConfigLoaded = true;
        await loadFullConfigPanel();
      }
    });
  });

  // -- Library Back --
  const backBtn = document.getElementById('libBack');
  if (backBtn) {
    backBtn.addEventListener('click', async () => {
      const parts = (libraryPath || '').split('/').filter(Boolean);
      parts.pop();
      libraryPath = parts.join('/');
      try { await refreshLibrary(); } catch (err) { alert(err); }
    });
  }

  // -- Search --
  const searchForm = document.getElementById('searchForm');
  if (searchForm) {
    searchForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const q = document.getElementById('q').value.trim();
      const hint = document.getElementById('searchHint');
      if (hint) hint.textContent = '';

      const bookId = parseBookId(q);
      if (bookId) {
        try {
          await startDownload(bookId);
          if (hint) hint.textContent = `已创建下载任务：${bookId}`;
          const out = document.getElementById('searchResults');
          if (out) out.innerHTML = '<tr class="empty-row"><td colspan="4">已加入任务队列，可在"任务"页查看进度</td></tr>';
        } catch (err) {
          if (hint) hint.textContent = '创建任务失败';
          alert(err);
        }
        return;
      }
      try { await doSearch(q); } catch (err) { alert(err); }
    });
  }

  // -- Updates --
  const updBtn = document.getElementById('updatesRefresh');
  if (updBtn) updBtn.addEventListener('click', async () => {
    try { await refreshUpdates(); } catch (err) { alert(err); }
  });

  // -- App Update --
  const appUpdBtn = document.getElementById('appUpdateCheck');
  if (appUpdBtn) appUpdBtn.addEventListener('click', async () => {
    try { await refreshAppUpdate(true); } catch (err) { alert(err); }
  });

  const dismissBtn = document.getElementById('appUpdateDismiss');
  if (dismissBtn) dismissBtn.addEventListener('click', async () => {
    try {
      const { latestTag } = await refreshAppUpdate(false);
      if (latestTag) {
        setDismissedTag(latestTag);
        showAppUpdateBanner(false);
        const hint = document.getElementById('appUpdateHint');
        if (hint) hint.textContent = '已设置不再提醒';
      }
    } catch (err) { alert(err); }
  });

  const selfUpdBtn = document.getElementById('appSelfUpdate');
  if (selfUpdBtn) selfUpdBtn.addEventListener('click', async () => {
    const hint = document.getElementById('appUpdateHint');
    if (hint) hint.textContent = '自更新启动中…';
    try {
      await j('/api/self_update', { method: 'POST' });
      if (hint) hint.textContent = '已触发自更新，服务将重启';
    } catch (err) {
      if (hint) hint.textContent = '自更新触发失败';
      alert(err);
    }
  });

  // -- Quick Config Save --
  const cfgForm = document.getElementById('configForm');
  if (cfgForm) cfgForm.addEventListener('submit', async (e) => {
    e.preventDefault();
    const msg = document.getElementById('configMsg');
    if (msg) msg.textContent = '保存中…';
    try {
      await saveConfig();
      if (msg) msg.textContent = '已保存';
    } catch (err) {
      if (msg) msg.textContent = '保存失败';
      alert(err);
    }
  });

  // -- Full Config Save --
  const cfgFullSave = document.getElementById('cfgFullSave');
  if (cfgFullSave) cfgFullSave.addEventListener('click', async () => {
    const msg = document.getElementById('cfgFullMsg');
    if (msg) msg.textContent = '保存中…';
    try {
      await saveFullConfig();
      await refreshConfig();
      await refreshRawConfig();
      if (msg) msg.textContent = '已保存';
    } catch (err) {
      if (msg) msg.textContent = '保存失败';
      alert(err);
    }
  });

  // -- YAML Config --
  const cfgRawReload = document.getElementById('cfgRawReload');
  if (cfgRawReload) cfgRawReload.addEventListener('click', async () => {
    const msg = document.getElementById('cfgRawMsg');
    if (msg) msg.textContent = '加载中…';
    try {
      await refreshRawConfig();
      if (msg) msg.textContent = '已加载';
    } catch (err) {
      if (msg) msg.textContent = '加载失败';
      alert(err);
    }
  });

  const cfgRawSave = document.getElementById('cfgRawSave');
  if (cfgRawSave) cfgRawSave.addEventListener('click', async () => {
    const msg = document.getElementById('cfgRawMsg');
    if (msg) msg.textContent = '保存中…';
    try {
      await saveRawConfig();
      await refreshConfig();
      await refreshRawConfig();
      if (msg) msg.textContent = '已保存';
    } catch (err) {
      if (msg) msg.textContent = '保存失败';
      alert(err);
    }
  });

  // -- Delegated Click Handlers --
  document.addEventListener('click', async (e) => {
    const t = e.target;
    if (!t || !t.classList) return;

    if (t.classList.contains('startDownload')) {
      const bookId = t.getAttribute('data-bookid');
      try { await startDownload(bookId); } catch (err) { alert(err); }
    }
    if (t.classList.contains('cancelJob')) {
      const id = t.getAttribute('data-jobid');
      if (!confirm('确认取消该任务并从列表中清理吗？')) return;
      try { await cancelJob(id); } catch (err) { alert(err); }
    }
    if (t.classList.contains('retryJob')) {
      const bookId = t.getAttribute('data-bookid');
      const jobId = t.getAttribute('data-jobid');
      try {
        await startDownloadDirect(bookId);
        if (jobId) {
          await clearJob(jobId).catch(() => {});
        }
        await refreshJobs();
      } catch (err) { alert(err); }
    }
    if (t.classList.contains('goLibrary')) {
      const title = t.getAttribute('data-title') || '';
      const jobId = t.getAttribute('data-jobid');
      if (jobId) {
        await clearJob(jobId).catch(() => {});
        await refreshJobs().catch(() => {});
      }
      libraryPath = '';
      window.location.hash = '#library';
      await refreshLibrary();
      highlightLibraryItem(title);
    }
    if (t.classList.contains('openDir')) {
      const p = (t.getAttribute('data-path') || '').toString();
      libraryPath = p;
      try { await refreshLibrary(); } catch (err) { alert(err); }
    }
  });

  // -- Escape Key for Modals --
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      const previewModal = document.getElementById('previewModal');
      if (previewModal && !previewModal.classList.contains('hidden')) {
        showPreviewModal(false);
        return;
      }
      const loginModal = document.getElementById('loginModal');
      if (loginModal && !loginModal.classList.contains('hidden')) {
        showLogin(false);
      }
    }
  });

  // -- Preview Modal Buttons --
  const previewConfirm = document.getElementById('previewConfirm');
  if (previewConfirm) previewConfirm.addEventListener('click', async () => {
    try { await confirmPreview(); } catch (err) { alert(err); }
  });

  const previewCancel = document.getElementById('previewCancel');
  if (previewCancel) previewCancel.addEventListener('click', () => showPreviewModal(false));

  const previewClose = document.getElementById('previewClose');
  if (previewClose) previewClose.addEventListener('click', () => showPreviewModal(false));

  // -- Book Name Modal --
  const bookNameConfirm = document.getElementById('bookNameConfirm');
  if (bookNameConfirm) bookNameConfirm.addEventListener('click', async () => {
    const selected = document.querySelector('input[name="bookNameOpt"]:checked');
    if (!selected) { alert('请选择一个书名'); return; }
    await submitBookNameChoice(selected.value);
  });
}

function highlightLibraryItem(title) {
  if (!title) return;
  const rows = document.querySelectorAll('#libraryBody tr');
  for (const row of rows) {
    const firstTd = row.querySelector('td');
    if (firstTd && firstTd.textContent.includes(title)) {
      row.classList.add('lib-highlight');
      row.scrollIntoView({ behavior: 'smooth', block: 'center' });
      setTimeout(() => row.classList.remove('lib-highlight'), 3000);
      break;
    }
  }
}

// ── Boot ───────────────────────────────────────────────────────────

async function boot() {
  wire();
  await refreshStatus();
  if (!isDockerBuild) await refreshAppUpdate(false).catch(() => {});
  await refreshConfig();
  await refreshRawConfig();
  await refreshUpdates();
  await refreshJobs();
  await refreshLibrary();
  setInterval(() => refreshJobs().catch(() => {}), 1500);
  setInterval(() => refreshStatus().catch(() => {}), 5000);
  if (!isDockerBuild) {
    setInterval(() => refreshAppUpdate(false).catch(() => {}), 6 * 60 * 60 * 1000);
  }
}

boot().catch(err => console.error(err));
