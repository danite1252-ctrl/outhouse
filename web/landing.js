// Outhouse landing page: download buttons, scroll reveals and a bit of
// ambience. No libraries. The page works without this file too.

// ---------- where the downloads live ----------
// The one place to change if the app downloads ever move. deploy/fetch-updates.py
// mirrors the newest builds from GitHub into /updates/ on this server.
const DOWNLOADS = {
  base: "/updates/",
  windows: { file: "Outhouse-Windows-Setup.exe", manifest: "latest.json" },
  android: { file: "Outhouse-Android.apk", manifest: "android.json" },
};

(() => {
  "use strict";
  // The inline script in <head> is already sending this visitor on to /app.
  if (!document.documentElement.classList.contains("js")) return;
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

  // ---------- downloads ----------
  const sizeText = (bytes) => {
    const n = Number(bytes);
    if (!n) return "";
    return n >= 1e6 ? `${Math.round(n / 1e6)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`;
  };

  async function checkDownload(kind) {
    const conf = DOWNLOADS[kind];
    const btn = $(`[data-download="${kind}"]`);
    const meta = $(`[data-version="${kind}"]`);
    if (!conf || !btn) return;
    const url = DOWNLOADS.base + conf.file;
    let size = "";
    let ok = false;
    try {
      const res = await fetch(url, { method: "HEAD", cache: "no-store" });
      ok = res.ok;
      size = sizeText(res.headers.get("content-length"));
    } catch { /* offline or blocked */ }
    if (!ok) {
      btn.textContent = "Coming soon";
      btn.classList.add("soon");
      btn.setAttribute("aria-disabled", "true");
      btn.removeAttribute("href");
      btn.closest(".dl-card")?.classList.add("unavailable");
      return;
    }
    btn.href = url;
    btn.setAttribute("download", conf.file);
    btn.removeAttribute("aria-disabled");
    btn.textContent = kind === "windows" ? "Download for Windows" : "Download for Android";
    let version = "";
    try {
      const res = await fetch(DOWNLOADS.base + conf.manifest, { cache: "no-store" });
      if (res.ok) version = String((await res.json()).version || "");
    } catch { /* no manifest: fine */ }
    if (meta) {
      const bits = [];
      if (version) bits.push(`<span class="ver">Version ${version.replace(/[^\w.+-]/g, "")}</span>`);
      bits.push(kind === "windows" ? "Windows 10 and 11" : "Android 7 and newer");
      if (size) bits.push(size);
      meta.innerHTML = bits.join(" · ");
    }
  }
  checkDownload("windows");
  checkDownload("android");

  // Point out the right download for this device.
  const ua = navigator.userAgent;
  const platform = /Android/i.test(ua) ? "android"
    : /iPhone|iPad|iPod|Macintosh/i.test(ua) && "ontouchend" in document ? "web"
    : /Windows/i.test(ua) ? "windows" : "";
  if (platform) $(`.dl-card[data-platform="${platform}"]`)?.classList.add("recommended");

  // "Install as an app" where the browser offers it.
  let installPrompt = null;
  const installBtn = $("#install-btn");
  addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    installPrompt = e;
    if (installBtn) installBtn.hidden = false;
  });
  installBtn?.addEventListener("click", async () => {
    if (!installPrompt) return;
    installPrompt.prompt();
    await installPrompt.userChoice.catch(() => null);
    installPrompt = null;
    installBtn.hidden = true;
  });

  // ---------- the raid calendar in the mock ----------
  const cal = $(".cal-grid");
  if (cal) {
    const now = new Date();
    const year = now.getFullYear(), month = now.getMonth();
    const first = (new Date(year, month, 1).getDay() + 6) % 7; // Monday first
    const days = new Date(year, month + 1, 0).getDate();
    $(".cal-h").innerHTML = `<span>${now.toLocaleString("en-AU", { month: "long" })} ${year}</span><span>Raids: Wed · Fri</span>`;
    const html = ["M", "T", "W", "T", "F", "S", "S"].map((d) => `<span class="wd">${d}</span>`);
    for (let i = 0; i < first; i++) html.push('<span class="empty"></span>');
    for (let d = 1; d <= days; d++) {
      const wd = (first + d - 1) % 7;
      const cls = [wd === 2 || wd === 4 ? "raid" : "", d === now.getDate() ? "today" : ""].join(" ").trim();
      html.push(`<span${cls ? ` class="${cls}"` : ""}>${d}</span>`);
    }
    cal.innerHTML = html.join("");
  }

  // ---------- reveal on scroll ----------
  const revealables = $$(".reveal, .window, .chatmock, .raidmock, .card");
  if ("IntersectionObserver" in window && !reduced) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (e.isIntersecting) {
          e.target.classList.add("in");
          io.unobserve(e.target);
        }
      }
    }, { rootMargin: "0px 0px -8% 0px", threshold: 0.12 });
    revealables.forEach((n) => io.observe(n));
  } else {
    revealables.forEach((n) => n.classList.add("in"));
  }

  // ---------- the hero: door, embers, parallax ----------
  const stage = $("#stage");
  const embers = $("#embers");
  if (embers && !reduced) {
    const frag = document.createDocumentFragment();
    const count = innerWidth < 560 ? 12 : 20;
    for (let i = 0; i < count; i++) {
      const s = document.createElement("span");
      const r = (a, b) => (a + Math.random() * (b - a)).toFixed(2);
      s.style.cssText = `--x:${r(18, 82)}%;--s:${r(2.5, 5.5)}px;--t:${r(6, 12)}s;--d:${r(0, 9)}s;--dx:${r(-60, 60)}px`;
      frag.appendChild(s);
    }
    embers.appendChild(frag);
  }
  if (stage) requestAnimationFrame(() => setTimeout(() => stage.classList.add("open"), reduced ? 0 : 450));

  const nav = $("#nav");
  let ticking = false;
  const onScroll = () => {
    if (ticking) return;
    ticking = true;
    requestAnimationFrame(() => {
      const y = scrollY;
      nav?.classList.toggle("scrolled", y > 8);
      if (!reduced && y < 1200) document.documentElement.style.setProperty("--scroll", y.toFixed(0));
      ticking = false;
    });
  };
  addEventListener("scroll", onScroll, { passive: true });
  onScroll();

  if (stage && !reduced && matchMedia("(pointer: fine)").matches) {
    let raf = 0;
    addEventListener("pointermove", (e) => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        const px = (e.clientX / innerWidth - 0.5) * 2;
        const py = (e.clientY / innerHeight - 0.5) * 2;
        stage.style.setProperty("--px", px.toFixed(3));
        stage.style.setProperty("--py", py.toFixed(3));
      });
    }, { passive: true });
  }
})();
