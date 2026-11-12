'use strict';

const $ = (id) => document.getElementById(id);

// ---- download ----------------------------------------------------------------------------
// Picks the installer for the visitor's system from the latest release. Installer names carry
// the version (Orbitbar_0.2.0_x64-setup.exe), so they come from the GitHub API at load time.
// Without this script, or when the API is unreachable, every link opens the latest release page.
(() => {
  const REPO = 'montesgp/orbitbar';
  const LATEST = `https://github.com/${REPO}/releases/latest`;
  // Each system lists its builds, the default first. A build for another architecture is offered
  // only when the release has its installer.
  const SYSTEMS = {
    windows: {
      name: 'Windows',
      builds: [
        {
          arch: 'x64',
          detail: 'Windows 10 and 11 · installer',
          label: 'installer (.exe)',
          installer: /_x64-setup\.exe$/,
          formats: [{ label: '.msi', pattern: /_x64_[\w-]+\.msi$/ }],
        },
        {
          arch: 'arm64',
          detail: 'Windows 11 on Arm · installer',
          label: 'installer (.exe)',
          installer: /_arm64-setup\.exe$/,
          formats: [{ label: '.msi', pattern: /_arm64_[\w-]+\.msi$/ }],
        },
      ],
    },
    macos: {
      name: 'macOS',
      builds: [
        {
          arch: 'universal',
          detail: 'Apple silicon and Intel · .dmg',
          label: '.dmg',
          installer: /_universal\.dmg$/,
          formats: [],
        },
      ],
    },
    linux: {
      name: 'Linux',
      builds: [
        {
          arch: 'x64',
          detail: 'x64 · AppImage',
          label: 'AppImage',
          installer: /_amd64\.AppImage$/,
          formats: [
            { label: '.deb (Debian, Ubuntu)', pattern: /_amd64\.deb$/ },
            { label: '.rpm (Fedora, openSUSE)', pattern: /\.x86_64\.rpm$/ },
          ],
        },
        {
          arch: 'arm64',
          detail: 'ARM64 · AppImage',
          label: 'AppImage',
          installer: /_aarch64\.AppImage$/,
          formats: [
            { label: '.deb (Debian, Ubuntu)', pattern: /_arm64\.deb$/ },
            { label: '.rpm (Fedora, openSUSE)', pattern: /\.aarch64\.rpm$/ },
          ],
        },
      ],
    },
  };
  const ARCH_NAMES = { x64: 'x64', arm64: 'ARM64' };
  const KEYS = Object.keys(SYSTEMS);
  let assets = [];

  /** `windows`, `macos`, `linux`, `mobile`, or null when unknown. */
  function detectSystem() {
    const hints = navigator.userAgentData;
    if (hints && hints.mobile) return 'mobile';
    const platform = ((hints && hints.platform) || '').toLowerCase();
    const agent = (navigator.userAgent || '').toLowerCase();
    if (/android|iphone|ipad|ipod|cros|chrome os/.test(`${platform} ${agent}`)) return 'mobile';
    // Client hints name the platform exactly; the user agent string is the fallback (Safari, Firefox).
    const text = platform || agent;
    // macOS first: "Darwin" contains "win".
    // iPads also say "Macintosh"; Macs have no touch screen.
    if (/mac/.test(text)) return navigator.maxTouchPoints > 1 ? 'mobile' : 'macos';
    if (/win/.test(text)) return 'windows';
    if (/linux|x11/.test(text)) return 'linux';
    return null;
  }

  /**
   * `arm64`, `x64`, or null when unknown. Client hints (Chrome, Edge) report the architecture even
   * where the user agent string is frozen to x64; Firefox on Linux names it in the user agent.
   * Firefox and Safari on Windows give no hint, so those visitors keep the x64 default.
   */
  async function detectArch() {
    const hints = navigator.userAgentData;
    if (hints && typeof hints.getHighEntropyValues === 'function') {
      try {
        const { architecture, bitness } = await hints.getHighEntropyValues(['architecture', 'bitness']);
        // 32-bit ARM (older Raspberry Pi systems) cannot run the ARM64 build.
        if (architecture === 'arm') return bitness === '32' ? null : 'arm64';
        if (architecture === 'x86') return 'x64';
      } catch {
        // Hints refused: fall back to the user agent string.
      }
    }
    return /aarch64|arm64/.test((navigator.userAgent || '').toLowerCase()) ? 'arm64' : null;
  }

  /** The download URL of the first asset whose name matches, or the latest release page. */
  function urlFor(pattern) {
    const asset = assets.find((a) => pattern.test(a.name));
    return asset ? asset.url : LATEST;
  }

  const released = (pattern) => assets.some((a) => pattern.test(a.name));

  /** The build for the visitor's architecture when the release has it, else the default build. */
  function pickBuild(system, arch) {
    return system.builds.find((b) => b.arch === arch && released(b.installer)) || system.builds[0];
  }

  /** Links to a formats list, keeping only the files the release has. */
  function formatLinks(formats) {
    return formats
      .filter((f) => released(f.pattern))
      .flatMap((f, i) => [...(i ? [' · '] : []), link(urlFor(f.pattern), f.label)]);
  }

  function link(href, text) {
    const a = document.createElement('a');
    a.href = href;
    a.textContent = text;
    return a;
  }

  function render(key, arch) {
    const system = SYSTEMS[key];
    const build = system ? pickBuild(system, arch) : null;
    const button = $('download');
    if (build) {
      button.href = urlFor(build.installer);
      $('download-os').textContent = `for ${system.name}`;
      $('download-detail').textContent = build.detail;
    }

    // Other formats of the same build, once the release says which files exist.
    const formats = build ? formatLinks(build.formats) : [];
    $('formats-links').replaceChildren(...formats);
    $('formats').hidden = formats.length === 0;

    // The same system for the other architecture: ARM64 when the visitor's is unknown or x64, so
    // nobody is left without a download, and x64 for visitors given the ARM64 build.
    const other = system ? system.builds.find((b) => b !== build && released(b.installer)) : null;
    if (other) {
      $('arch-name').textContent = ARCH_NAMES[other.arch];
      const installer = link(urlFor(other.installer), other.label);
      const rest = formatLinks(other.formats);
      $('arch-links').replaceChildren(installer, ...(rest.length ? [' · ', ...rest] : []));
    }
    $('arch').hidden = !other;

    // The other systems, or all three when the visitor's is unknown.
    const others = KEYS.filter((k) => k !== key);
    KEYS.forEach((k) => {
      const anchor = $(`link-${k}`);
      anchor.hidden = k === key;
      anchor.href = urlFor(SYSTEMS[k].builds[0].installer);
      // A separator follows every shown link except the last one.
      const separator = $(`sep-${k}`);
      if (separator) separator.hidden = k === key || k === others[others.length - 1];
    });
    $('others-prefix').textContent = key ? 'Also for' : 'Available for';
    $('others').hidden = false;
  }

  function showHelp(key) {
    KEYS.forEach((k) => { $(`help-${k}`).open = k === key; });
  }

  async function loadLatestRelease(key, archPromise) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 8000);
    try {
      const response = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
        headers: { Accept: 'application/vnd.github+json' },
        signal: controller.signal,
      });
      if (!response.ok) return;
      const release = await response.json();
      const tag = typeof release.tag_name === 'string' ? release.tag_name : '';
      const url = typeof release.html_url === 'string' ? release.html_url : '';
      if (!/^v\d+\.\d+\.\d+$/.test(tag) || !url.startsWith(`https://github.com/${REPO}/releases/`)) return;

      // Only files of this release, served by GitHub, become download links.
      const prefix = `https://github.com/${REPO}/releases/download/${tag}/`;
      assets = (Array.isArray(release.assets) ? release.assets : [])
        .map((a) => ({ name: String(a && a.name), url: String(a && a.browser_download_url) }))
        .filter((a) => a.url === prefix + encodeURIComponent(a.name) || a.url === prefix + a.name);
      render(key, await archPromise);

      $('release-tag').textContent = tag;
      $('release-link').href = url;
      const date = new Date(release.published_at);
      if (!Number.isNaN(date.getTime())) {
        const day = date.toLocaleDateString('en', { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' });
        $('release-date').textContent = ` · ${day}`;
      }
      $('release').hidden = false;
    } catch {
      // Offline, rate-limited or blocked: the links still open the latest release page.
    } finally {
      clearTimeout(timer);
    }
  }

  const detected = detectSystem();
  const key = SYSTEMS[detected] ? detected : null;
  $('desktop-only').hidden = detected !== 'mobile';
  render(key, null);
  showHelp(key || 'windows');
  // Asked in parallel with the release; detectArch never rejects.
  loadLatestRelease(key, detectArch());
})();

// ---- animated preview ----------------------------------------------------------------------
// A scripted tour of the bar drawn in HTML. It starts paused for visitors who prefer reduced
// motion, and pauses itself while it is off screen.
(() => {
  const frame = $('demo-frame');
  const stage = $('stage');
  if (!frame || !stage) return;

  const ob = $('ob');
  const panel = $('ob-panel');
  const sections = $('ob-sections');
  const options = $('ob-options');
  const menu = $('ob-menu');
  const cursor = $('cursor');
  const term = $('term-body');
  const toggle = $('demo-toggle');
  const cell = (name) => stage.querySelector(`[data-cell="${name}"]`);

  // Example numbers in the panel's own format (see app/src/usage-view.ts).
  const USAGE = {
    today: {
      label: 'Today',
      claude: {
        name: 'Claude', line: 'in 1.9K out 211.3K cache 98.42M/2.14M', sub: 'reasoning 31.2K · 612 entries',
        cost: 53.04, api: true, rows: [['my-app', '180.4K', 45.10], ['api-service', '30.9K', 7.94]],
      },
      codex: {
        name: 'Codex', line: 'in 6.12M out 24.7K cache 5.87M/0', sub: 'reasoning 9.8K · 2 entries',
        cost: 2.31, api: true, rows: [['web-store', '24.7K', 2.31]],
      },
      opencode: {
        name: 'OpenCode', line: 'in 1.38M out 203.5K cache 26.10M/0', sub: 'reasoning 61.0K · 4 entries',
        cost: 0, api: false, rows: [['cli-tool', '203.5K', 0]],
      },
    },
    week: {
      label: '7 days',
      claude: {
        name: 'Claude', line: 'in 11.5K out 1.42M cache 1207.10M/20.37M', sub: 'reasoning 222.7K · 5.7K entries',
        cost: 317.67, api: true, rows: [['my-app', '905.1K', 218.52], ['api-service', '413.9K', 83.87], ['docs-site', '101.4K', 15.28]],
      },
      codex: {
        name: 'Codex', line: 'in 57.88M out 192.4K cache 56.33M/0', sub: 'reasoning 65.7K · 8 entries',
        cost: 16.67, api: true, rows: [['web-store', '192.4K', 16.67]],
      },
      opencode: {
        name: 'OpenCode', line: 'in 9.60M out 1.55M cache 209.04M/0', sub: 'reasoning 498.4K · 25 entries',
        cost: 0, api: false, rows: [['api-service', '1.38M', 0], ['cli-tool', '173.6K', 0]],
      },
    },
  };
  const ALL = ['claude', 'codex', 'opencode'];

  const TERM_LINES = [
    ['dot', 'Running npm test'],
    ['t-dim', '  ✓ 48 passed (3.1s)'],
    ['t-tokens', '  ↓ 18.4K tokens · 2m 11s'],
    ['t-prompt', '> also document the new limit in the README'],
    ['dot', 'Reading README.md'],
    ['dot', 'Editing README.md'],
    ['t-tokens', '  ↓ 6.2K tokens · 41s'],
    ['t-prompt', '> commit it'],
    ['dot', 'git commit -m "feat: rate limit the login endpoint"'],
    ['t-dim', '  1 commit, 3 files changed'],
  ];

  // ---- timing that stops while paused ----
  const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
  let userPaused = reduceMotion.matches;
  let onScreen = true;
  let started = false;
  const paused = () => userPaused || !onScreen || document.hidden;

  function frames(ms, step) {
    return new Promise((resolve) => {
      let last = performance.now();
      let elapsed = 0;
      const tick = (now) => {
        // A long gap (background tab) counts as one frame, so the tour resumes where it stopped.
        if (!paused()) elapsed += Math.min(now - last, 50);
        last = now;
        const t = Math.min(elapsed / ms, 1);
        if (step) step(t);
        if (t >= 1) resolve();
        else requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
  }

  const wait = (ms) => frames(ms);
  const ease = (t) => (t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2);

  // ---- scaling ----
  const fit = () => stage.style.setProperty('--scale', String(frame.clientWidth / 960));
  fit();
  if ('ResizeObserver' in window) new ResizeObserver(fit).observe(frame);
  else window.addEventListener('resize', fit);

  // ---- cursor ----
  let cx = 430;
  let cy = 470;
  const placeCursor = () => { cursor.style.transform = `translate(${cx}px, ${cy}px)`; };

  /** Stage coordinates of a point inside an element (fx, fy are fractions of its box). */
  function pointIn(el, fx = 0.5, fy = 0.5) {
    const scale = frame.clientWidth / 960 || 1;
    const r = el.getBoundingClientRect();
    const s = stage.getBoundingClientRect();
    return [(r.left - s.left + r.width * fx) / scale, (r.top - s.top + r.height * fy) / scale];
  }

  async function moveTo(el, ms = 800, fx = 0.5, fy = 0.5) {
    const [x0, y0] = [cx, cy];
    const [x1, y1] = pointIn(el, fx, fy);
    await frames(ms, (t) => {
      const e = ease(t);
      cx = x0 + (x1 - x0) * e;
      cy = y0 + (y1 - y0) * e;
      placeCursor();
    });
  }

  async function click(el) {
    cursor.classList.remove('click');
    void cursor.getBoundingClientRect();
    cursor.classList.add('click');
    if (el) {
      el.classList.add('press');
      await wait(140);
      el.classList.remove('press');
    }
  }

  /** Moves to an element, hovers it, clicks it, and leaves the hover once the cursor moves on. */
  async function press(el, ms) {
    await moveTo(el, ms);
    el.classList.add('hover');
    await wait(220);
    await click(el);
    return () => el.classList.remove('hover');
  }

  // ---- panel ----
  const money = (value, api) => (api ? `≈ $${value.toFixed(2)} API` : `$${value.toFixed(2)}`);

  function p(text, cls) {
    const el = document.createElement('p');
    if (cls) el.className = cls;
    el.textContent = text;
    return el;
  }

  /** Fills the panel with one window's numbers; costs count up from zero when `count` is set. */
  async function showUsage(windowKey, agents, count) {
    const data = USAGE[windowKey];
    $('ob-window').textContent = data.label;
    const counters = [];
    sections.replaceChildren(...agents.map((key) => {
      const agent = data[key];
      const section = document.createElement('div');
      section.className = 'ob-section';
      const cost = p(money(count ? 0 : agent.cost, agent.api), 'ob-cost');
      counters.push([cost, agent.cost, agent.api]);
      section.append(p(agent.name, 'ob-agent'), p(agent.line), p(agent.sub, 'ob-dim'), cost);
      agent.rows.forEach(([project, tokens, value]) => {
        const row = document.createElement('p');
        row.className = 'ob-row';
        const name = document.createElement('span');
        name.textContent = project;
        const stats = document.createElement('span');
        stats.textContent = `${tokens} ${money(value, agent.api)}`;
        row.append(name, stats);
        section.append(row);
      });
      return section;
    }));
    if (count) {
      await frames(700, (t) => {
        const e = ease(t);
        counters.forEach(([el, value, api]) => { el.textContent = money(value * e, api); });
      });
    }
  }

  async function swapUsage(windowKey, agents) {
    sections.classList.add('swap');
    await wait(160);
    const done = showUsage(windowKey, agents, true);
    sections.classList.remove('swap');
    await done;
  }

  function setTheme(name) {
    ob.classList.toggle('light', name === 'light');
    menu.querySelectorAll('[data-check]').forEach((el) => {
      el.textContent = el.dataset.check === name ? '✓' : '';
    });
  }

  // ---- terminal ----
  const termStart = Array.from(term.children, (el) => el.cloneNode(true));
  let termNext = 0;

  function termLine() {
    const [kind, text] = TERM_LINES[termNext % TERM_LINES.length];
    termNext += 1;
    const line = document.createElement('p');
    line.className = 'new';
    if (kind === 'dot') {
      const dot = document.createElement('span');
      dot.className = 't-dot';
      dot.textContent = '●';
      line.append(dot, ` ${text}`);
    } else {
      line.classList.add(kind);
      line.textContent = text;
    }
    term.append(line);
    while (term.children.length > 15) term.firstElementChild.remove();
  }

  function reset() {
    ob.classList.remove('light', 'collapsed');
    setTheme('dark');
    panel.classList.add('closed');
    menu.classList.remove('open');
    options.classList.remove('open');
    stage.querySelectorAll('.hover').forEach((el) => el.classList.remove('hover'));
    term.replaceChildren(...termStart.map((el) => el.cloneNode(true)));
    termNext = 0;
  }

  // ---- the tour ----
  async function tour() {
    stage.classList.remove('static');
    for (;;) {
      reset();
      await wait(900);
      termLine();

      // The $ cell opens every agent, on Today.
      let leave = await press(cell('usage'));
      await showUsage('today', ALL, false);
      panel.classList.remove('closed');
      await showUsage('today', ALL, true);
      await wait(1200);
      termLine();
      await wait(800);

      // Switch the window to 7 days.
      leave();
      leave = await press($('ob-select'), 900);
      options.classList.add('open');
      await wait(450);
      leave();
      const week = options.querySelector('[data-window="week"]');
      leave = await press(week, 450);
      options.classList.remove('open');
      leave();
      await swapUsage('week', ALL);
      await wait(1600);
      termLine();
      termLine();
      await wait(900);

      // One agent's cell keeps the window and shows only that agent.
      leave = await press(cell('claude'), 900);
      await swapUsage('week', ['claude']);
      await wait(2000);
      termLine();

      // The settings cell opens the menu; pick the Light theme.
      leave();
      leave = await press(cell('settings'), 900);
      panel.classList.add('closed');
      menu.classList.add('open');
      await wait(600);
      leave();
      leave = await press(menu.querySelector('[data-item="light"]'), 700);
      leave();
      menu.classList.remove('open');
      setTheme('light');
      await wait(700);
      termLine();

      // The usage panel in the light theme.
      leave = await press(cell('usage'), 900);
      await showUsage('today', ALL, false);
      panel.classList.remove('closed');
      await showUsage('today', ALL, true);
      await wait(2000);
      termLine();

      // Collapse to a tab, then bring it back.
      leave();
      leave = await press($('ob-collapse'), 900);
      leave();
      panel.classList.add('closed');
      ob.classList.add('collapsed');
      await wait(500);
      await moveTo(frame, 900, 0.5, 0.62);
      await wait(1400);
      termLine();
      leave = await press($('ob-tab'), 900);
      leave();
      ob.classList.remove('collapsed');
      await wait(900);

      // Back to Dark for the next round.
      leave = await press(cell('settings'), 700);
      menu.classList.add('open');
      await wait(500);
      leave();
      leave = await press(menu.querySelector('[data-item="dark"]'), 600);
      leave();
      menu.classList.remove('open');
      setTheme('dark');
      await moveTo(frame, 1000, 0.45, 0.85);
      await wait(1800);
    }
  }

  function start() {
    if (started) return;
    started = true;
    tour();
  }

  toggle.hidden = false;
  const label = () => { toggle.textContent = userPaused ? 'Play' : 'Pause'; };
  label();
  toggle.addEventListener('click', () => {
    userPaused = !userPaused;
    label();
    if (!userPaused) start();
  });

  if ('IntersectionObserver' in window) {
    new IntersectionObserver((entries) => {
      onScreen = entries[entries.length - 1].isIntersecting;
    }).observe(frame);
  }

  // Visitors who prefer reduced motion see the still frame until they press Play.
  if (!userPaused) start();
})();
