let loginPromise = null;

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
    if (!form) {
      reject(new Error('login form missing'));
      return;
    }

    const handler = async (e) => {
      e.preventDefault();
      const pw = (document.getElementById('loginPassword')?.value || '').toString();
      try {
        const res = await fetch('/api/login', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ password: pw })
        });
        if (!res.ok) {
          if (msg) msg.textContent = '密码错误';
          return;
        }
        showLogin(false);
        form.removeEventListener('submit', handler);
        resolve(true);
      } catch (err) {
        if (msg) msg.textContent = String(err || 'login failed');
      }
    };

    form.addEventListener('submit', handler);
  }).finally(() => {
    loginPromise = null;
  });

  return loginPromise;
}

async function j(url, opts) {
  const res = await fetch(url, opts);
  if (res.status === 401) {
    await requireLogin();
    const res2 = await fetch(url, opts);
    if (!res2.ok) {
      const text = await res2.text().catch(() => "");
      throw new Error(`${res2.status} ${res2.statusText}${text ? `: ${text}` : ""}`);
    }
    const ct2 = res2.headers.get("content-type") || "";
    if (ct2.includes("application/json")) return res2.json();
    return res2.text();
  }
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`${res.status} ${res.statusText}${text ? `: ${text}` : ""}`);
  }
  const ct = res.headers.get("content-type") || "";
  if (ct.includes("application/json")) return res.json();
  return res.text();
}

function esc(s) {
  return (s ?? "").toString().replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));
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

function parseBookId(input) {
  const trimmed = (input ?? '').toString().trim();
  if (!trimmed) return '';

  // plain digits
  if (/^[0-9]+$/.test(trimmed)) return trimmed;

  // extract first URL if user pasted extra text around it
  const urlMatch = trimmed.match(/https?:\/\/\S+/i);
  const target = urlMatch ? urlMatch[0] : trimmed;

  // querystring: book_id=123 / bookId=123
  const qs = target.match(/(?:^|[?&#])(?:book_id|bookId)=([0-9]+)/i);
  if (qs && qs[1]) return qs[1];

  // path: /page/123
  const page = target.match(/\/page\/([0-9]+)/i);
  if (page && page[1]) return page[1];

  return '';
}

const DISMISS_KEY = 'tnd.dismissed_release_tag';

function getDismissedTag() {
  try { return (localStorage.getItem(DISMISS_KEY) || '').toString(); } catch { return ''; }
}

function setDismissedTag(tag) {
  try { localStorage.setItem(DISMISS_KEY, (tag || '').toString()); } catch {}
}

function showAppUpdateBanner(show) {
  const el = document.getElementById('appUpdateBanner');
  if (!el) return;
  el.classList.toggle('hidden', !show);
}

async function refreshAppUpdate(manual) {
  const hint = document.getElementById('appUpdateHint');
  const latestEl = document.getElementById('appUpdateLatest');
  const bodyEl = document.getElementById('appUpdateBody');
  const linkEl = document.getElementById('appUpdateLink');

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

let libraryPath = '';

async function refreshStatus() {
  const data = await j('/api/status');
  document.getElementById('version').textContent = data.version || '';
  document.getElementById('prewarm').textContent = data.prewarm_in_progress ? 'warming' : 'ready';
  document.getElementById('saveDir').textContent = data.save_dir || '';
  document.getElementById('bind').textContent = data.bind_addr || '';
  document.getElementById('locked').textContent = data.locked ? 'locked' : 'unlocked';
}

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
    body: JSON.stringify({
      novel_format: nf,
      bulk_files: bf,
      enable_audiobook: ea,
      audiobook_format: af,
    })
  });
}

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
    const hrefFile = `/download/${encodeURI(rel)}`;
    const hrefZip = `/download-zip/${encodeURI(rel)}`;

    const sizeText = kind === 'dir'
      ? `${fmtBytes(it.size)} (${Number(it.file_count || 0)} files)`
      : fmtBytes(it.size);

    const timeText = fmtTime(it.modified_ms);

    if (kind === 'dir') {
      tr.innerHTML = `
        <td><button class="openDir" data-path="${esc(rel)}">打开</button> ${esc(name)} <span class="badge">dir</span></td>
        <td>${esc(sizeText)}</td>
        <td>${esc(timeText)}</td>
        <td class="actions"><a href="${hrefZip}">打包下载</a></td>
      `;
    } else {
    tr.innerHTML = `
        <td><a href="${hrefFile}">${esc(name)}</a> <span class="badge">${esc(it.ext || '')}</span></td>
        <td>${esc(sizeText)}</td>
        <td>${esc(timeText)}</td>
        <td class="actions"><a href="${hrefFile}">下载</a></td>
    `;
    }
    tbody.appendChild(tr);
  }
  if (items.length === 0) {
    tbody.innerHTML = '<tr><td colspan="4" class="k">暂无文件（先下载一本书）。</td></tr>';
  }
}

async function doSearch(q) {
  const out = document.getElementById('searchResults');
  out.innerHTML = '';
  if (!q) return;
  const data = await j(`/api/search?q=${encodeURIComponent(q)}`);
  const items = data.items || [];
  if (items.length === 0) {
    out.innerHTML = '<tr><td colspan="4" class="k">无结果</td></tr>';
    return;
  }
  for (const b of items) {
    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td>${esc(b.title ?? '')}</td>
      <td>${esc(b.author ?? '')}</td>
      <td><code>${esc(b.book_id)}</code></td>
      <td><button data-bookid="${esc(b.book_id)}" class="startDownload">下载</button></td>
    `;
    out.appendChild(tr);
  }
}

let currentPreviewBookId = null;
let currentPreviewData = null;

function showPreviewModal(show) {
  const modal = document.getElementById('previewModal');
  if (!modal) return;
  modal.classList.toggle('hidden', !show);
  document.body.style.overflow = show ? 'hidden' : '';
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
  if (rangeHint) rangeHint.textContent = '';
  
  try {
    const preview = await j(`/api/preview/${encodeURIComponent(bookId)}`);
    currentPreviewData = preview;
    
    if (loading) loading.classList.add('hidden');
    if (data) data.classList.remove('hidden');
    
    // Update preview content
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
    
    if (author) {
      author.textContent = preview.author ? `作者: ${preview.author}` : '作者: 未知';
    }
    
    if (stats) {
      const parts = [];
      if (preview.chapter_count) parts.push(`章节: ${preview.chapter_count}`);
      if (preview.finished !== null && preview.finished !== undefined) {
        parts.push(`状态: ${preview.finished ? '完结' : '连载'}`);
      }
      if (preview.word_count) {
        const words = Number(preview.word_count);
        const wStr = words >= 10000 ? `${(words / 10000).toFixed(1)}万` : `${words}`;
        parts.push(`字数: ${wStr}字`);
      }
      if (preview.score) parts.push(`评分: ${preview.score.toFixed(1)}`);
      if (preview.read_count_text || preview.read_count) {
        parts.push(`阅读: ${preview.read_count_text || preview.read_count}`);
      }
      stats.innerHTML = parts.map(p => `<span>${esc(p)}</span>`).join('');
    }
    
    if (desc) {
      const description = preview.description || '暂无简介';
      desc.textContent = description;
    }
    
    if (tags) {
      if (preview.tags && preview.tags.length > 0) {
        tags.innerHTML = preview.tags.map(t => `<span class="badge">${esc(t)}</span>`).join('');
        tags.classList.remove('hidden');
      } else {
        tags.classList.add('hidden');
      }
    }
    
    if (chapters) {
      const chapterInfo = [];
      if (preview.chapter_count) {
        chapterInfo.push(`总章节数: ${preview.chapter_count}`);
      }
      if (preview.first_chapter_title) {
        chapterInfo.push(`首章: ${preview.first_chapter_title}`);
      }
      if (preview.last_chapter_title) {
        chapterInfo.push(`末章: ${preview.last_chapter_title}`);
      }
      if (preview.category) {
        chapterInfo.push(`分类: ${preview.category}`);
      }
      chapters.innerHTML = chapterInfo.map(i => `<div>${esc(i)}</div>`).join('');
    }
    
    if (cover) {
      const coverUrl = preview.detail_cover_url || preview.cover_url;
      if (coverUrl) {
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
  if (!currentPreviewBookId || !currentPreviewData) {
    showPreviewModal(false);
    return;
  }
  
  const rangeInput = document.getElementById('previewRangeInput');
  const rangeHint = document.getElementById('previewRangeHint');
  const rangeText = rangeInput ? rangeInput.value.trim() : '';
  
  let rangeStart = null;
  let rangeEnd = null;
  
  if (rangeText) {
    const parts = rangeText.split('-').map(p => p.trim());
    if (parts.length === 2) {
      const start = parseInt(parts[0], 10);
      const end = parseInt(parts[1], 10);
      const total = currentPreviewData.chapter_count || 0;
      
      if (isNaN(start) || isNaN(end) || start <= 0 || end <= 0 || start > end || end > total) {
        if (rangeHint) {
          rangeHint.textContent = `范围无效，请输入正确的范围 (1-${total})`;
          rangeHint.style.color = 'red';
        }
        return;
      }
      
      rangeStart = start;
      rangeEnd = end;
    } else if (rangeText) {
      if (rangeHint) {
        rangeHint.textContent = '格式应为 start-end，例如 1-10';
        rangeHint.style.color = 'red';
      }
      return;
    }
  }
  
  showPreviewModal(false);
  
  try {
    const payload = { book_id: currentPreviewBookId };
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
    
    const hint = document.getElementById('searchHint');
    if (hint) {
      if (rangeStart && rangeEnd) {
        hint.textContent = `已创建下载任务：${currentPreviewBookId} (章节 ${rangeStart}-${rangeEnd})`;
      } else {
        hint.textContent = `已创建下载任务：${currentPreviewBookId}`;
      }
    }
  } catch (err) {
    alert(`创建任务失败: ${err}`);
  }
}

function cancelPreview() {
  showPreviewModal(false);
  currentPreviewBookId = null;
  currentPreviewData = null;
}

async function startDownload(bookId) {
  // Open preview modal instead of directly downloading
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

async function refreshJobs() {
  const data = await j('/api/jobs');
  const tbody = document.getElementById('jobsBody');
  tbody.innerHTML = '';
  for (const it of data.items || []) {
    const tr = document.createElement('tr');
    const progress = it.progress ? `${it.progress.saved_chapters}/${it.progress.chapter_total}` : '';
    const title = it.title || it.book_id || '';
    tr.innerHTML = `
      <td><span class="badge">${esc(it.id)}</span></td>
      <td>${esc(title)}</td>
      <td>${esc(it.state || '')}</td>
      <td>${esc(progress)}</td>
      <td>
        <button data-jobid="${esc(it.id)}" class="cancelJob">取消</button>
      </td>
    `;
    tbody.appendChild(tr);
  }
  if ((data.items || []).length === 0) {
    tbody.innerHTML = '<tr><td colspan="5" class="k">暂无任务</td></tr>';
  }
}

async function refreshUpdates() {
  const hint = document.getElementById('updatesHint');
  const tbody = document.getElementById('updatesBody');
  if (!tbody) return;

  if (hint) hint.textContent = '扫描中…';
  tbody.innerHTML = '<tr><td colspan="7" class="k">加载中…</td></tr>';

  const data = await j('/api/updates');
  const updates = data.updates || [];
  const noUpdates = data.no_updates || [];
  const total = updates.length + noUpdates.length;

  if (hint) {
    hint.textContent = `可更新 ${updates.length} 本 / 无更新 ${noUpdates.length} 本 / 总计 ${total} 本`;
  }

  tbody.innerHTML = '';
  for (const it of updates) {
    const tr = document.createElement('tr');
    const title = it.book_name || '';
    const bookId = it.book_id || '';
    const localTotal = Number(it.local_total || 0);
    const remoteTotal = Number(it.remote_total || 0);
    const newCount = Number(it.new_count || 0);
    const failed = Number(it.local_failed || 0);
    tr.innerHTML = `
      <td>${esc(title)}</td>
      <td><code>${esc(bookId)}</code></td>
      <td>${esc(localTotal)}</td>
      <td>${esc(remoteTotal)}</td>
      <td>${esc(newCount)}</td>
      <td>${esc(failed)}</td>
      <td><button data-bookid="${esc(bookId)}" class="startDownload">更新</button></td>
    `;
    tbody.appendChild(tr);
  }

  if (updates.length === 0) {
    tbody.innerHTML = '<tr><td colspan="7" class="k">暂无可更新的小说。</td></tr>';
  }
}

async function cancelJob(id) {
  await j(`/api/jobs/${encodeURIComponent(id)}/cancel`, { method: 'POST' });
  await refreshJobs();
}

function wire() {
  const backBtn = document.getElementById('libBack');
  if (backBtn) {
    backBtn.addEventListener('click', async () => {
      const parts = (libraryPath || '').split('/').filter(Boolean);
      parts.pop();
      libraryPath = parts.join('/');
      try { await refreshLibrary(); } catch (err) { alert(err); }
    });
  }

  document.getElementById('searchForm').addEventListener('submit', async (e) => {
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
        if (out) out.innerHTML = '<tr><td colspan="4" class="k">已加入任务队列，可在“任务”页查看进度。</td></tr>';
      } catch (err) {
        if (hint) hint.textContent = '创建任务失败';
        alert(err);
      }
      return;
    }

    try { await doSearch(q); } catch (err) { alert(err); }
  });

  const updBtn = document.getElementById('updatesRefresh');
  if (updBtn) {
    updBtn.addEventListener('click', async () => {
      try { await refreshUpdates(); } catch (err) { alert(err); }
    });
  }

  const appUpdBtn = document.getElementById('appUpdateCheck');
  if (appUpdBtn) {
    appUpdBtn.addEventListener('click', async () => {
      try { await refreshAppUpdate(true); } catch (err) { alert(err); }
    });
  }

  const dismissBtn = document.getElementById('appUpdateDismiss');
  if (dismissBtn) {
    dismissBtn.addEventListener('click', async () => {
      try {
        const { latestTag } = await refreshAppUpdate(false);
        if (latestTag) {
          setDismissedTag(latestTag);
          showAppUpdateBanner(false);
          const hint = document.getElementById('appUpdateHint');
          if (hint) hint.textContent = '已设置不再提醒';
        }
      } catch (err) {
        alert(err);
      }
    });
  }

  const selfUpdBtn = document.getElementById('appSelfUpdate');
  if (selfUpdBtn) {
    selfUpdBtn.addEventListener('click', async () => {
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
  }

  const cfgForm = document.getElementById('configForm');
  if (cfgForm) {
    cfgForm.addEventListener('submit', async (e) => {
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
  }

  document.addEventListener('click', async (e) => {
    const t = e.target;
    if (t && t.classList && t.classList.contains('startDownload')) {
      const bookId = t.getAttribute('data-bookid');
      try { await startDownload(bookId); } catch (err) { alert(err); }
    }
    if (t && t.classList && t.classList.contains('cancelJob')) {
      const id = t.getAttribute('data-jobid');
      try { await cancelJob(id); } catch (err) { alert(err); }
    }

    if (t && t.classList && t.classList.contains('openDir')) {
      const p = (t.getAttribute('data-path') || '').toString();
      libraryPath = p;
      try { await refreshLibrary(); } catch (err) { alert(err); }
    }
  });
  
  // Wire up preview modal buttons
  const previewConfirm = document.getElementById('previewConfirm');
  if (previewConfirm) {
    previewConfirm.addEventListener('click', async () => {
      try { await confirmPreview(); } catch (err) { alert(err); }
    });
  }
  
  const previewCancel = document.getElementById('previewCancel');
  if (previewCancel) {
    previewCancel.addEventListener('click', () => {
      cancelPreview();
    });
  }
}

async function boot() {
  wire();
  await refreshStatus();
  await refreshAppUpdate(false).catch(() => {});
  await refreshConfig();
  await refreshUpdates();
  await refreshJobs();
  await refreshLibrary();
  setInterval(() => refreshJobs().catch(() => {}), 1500);
  setInterval(() => refreshStatus().catch(() => {}), 5000);
  setInterval(() => refreshAppUpdate(false).catch(() => {}), 6 * 60 * 60 * 1000);
}

boot().catch(err => {
  console.error(err);
});
