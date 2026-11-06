// brxswer shield — runtime layer. Runs at document start in every frame.
// Blocked calls are answered with plausible fakes so pages never notice.
(function () {
  'use strict';
  if (window.__brxShield) return;
  Object.defineProperty(window, '__brxShield', { value: true });

  var DOMAINS = new Set("__DOMAINS__");
  var HOST_PATHS = "__HOST_PATHS__";
  var PATH_CONTAINS = "__PATH_CONTAINS__";
  var PATH_SUFFIX = "__PATH_SUFFIX__";
  var BEACON_MODE = '__BEACON_MODE__';
  var KEY_FALLBACK = __KEY_FALLBACK__;
  var GIF = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';

  // ---------------------------------------------------------------- matching
  function hostHit(host) {
    var h = host;
    for (;;) {
      if (DOMAINS.has(h)) return h;
      var i = h.indexOf('.');
      if (i < 0) return null;
      h = h.slice(i + 1);
    }
  }
  function suffixOf(host, s) {
    return host === s || (host.length > s.length && host.slice(-s.length - 1) === '.' + s);
  }
  function verdict(raw) {
    var u;
    try { u = new URL(raw, location.href); } catch (e) { return null; }
    if (!/^(https?|wss?):$/.test(u.protocol)) return null;
    var host = u.hostname.toLowerCase(), path = u.pathname;
    var hit = hostHit(host);
    if (hit) return { host: host, rule: hit };
    for (var i = 0; i < HOST_PATHS.length; i++) {
      if (path.indexOf(HOST_PATHS[i][1]) === 0 && suffixOf(host, HOST_PATHS[i][0])) return { host: host, rule: HOST_PATHS[i][0] + HOST_PATHS[i][1] };
    }
    for (i = 0; i < PATH_CONTAINS.length; i++) if (path.indexOf(PATH_CONTAINS[i]) >= 0) return { host: host, rule: '*' + PATH_CONTAINS[i] };
    for (i = 0; i < PATH_SUFFIX.length; i++) {
      var s = PATH_SUFFIX[i];
      if (path.length >= s.length && path.slice(-s.length) === s) return { host: host, rule: '*' + s };
    }
    return null;
  }

  // --------------------------------------------------------------- reporting
  var queue = [], timer = 0;
  function report(layer, subject, detail) {
    queue.push([layer, String(subject).slice(0, 120), String(detail).slice(0, 120)]);
    if (!timer) timer = setTimeout(flush, 300);
  }
  function flush() {
    timer = 0;
    var batch = queue; queue = [];
    try { window.ipc.postMessage(JSON.stringify({ shield: batch })); } catch (e) {}
  }
  function pathOf(raw) {
    try { var u = new URL(raw, location.href); return u.pathname; } catch (e) { return ''; }
  }

  // ------------------------------------------------------------------- fetch
  var _fetch = window.fetch;
  if (_fetch) {
    window.fetch = function (input, init) {
      var url = typeof input === 'string' ? input : (input && input.url) || String(input);
      var v = verdict(url);
      if (v) {
        report('R', 'fetch → ' + v.host, pathOf(url) + ' → 204');
        return Promise.resolve(new Response(null, { status: 204, statusText: 'No Content' }));
      }
      return _fetch.apply(this, arguments);
    };
  }

  // --------------------------------------------------------------------- XHR
  var XO = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    var v = verdict(url);
    if (v) {
      report('R', 'xhr → ' + v.host, pathOf(url) + ' → empty');
      var args = Array.prototype.slice.call(arguments);
      args[0] = 'GET'; args[1] = 'data:text/plain,';
      return XO.apply(this, args);
    }
    return XO.apply(this, arguments);
  };

  // ------------------------------------------------------------------ beacon
  if (navigator.sendBeacon) {
    var SB = navigator.sendBeacon.bind(navigator);
    var noise = function (n) {
      var a = new Uint8Array(Math.max(8, Math.min(n || 64, 4096)));
      crypto.getRandomValues(a);
      var s = ''; for (var i = 0; i < a.length; i++) s += (a[i] % 36).toString(36);
      return s;
    };
    navigator.sendBeacon = function (url, data) {
      var v = verdict(url);
      if (v) { report('B', v.host, pathOf(url) + ' (dropped, faked true)'); return true; }
      if (BEACON_MODE === 'drop') { report('B', new URL(url, location.href).hostname, pathOf(url) + ' (dropped)'); return true; }
      var size = data == null ? 0 : (data.size || data.byteLength || String(data).length || 0);
      report('B', new URL(url, location.href).hostname, pathOf(url) + ' (payload scrambled)');
      return SB(url, noise(size));
    };
  }

  // ------------------------------------------------- pixels & script tags
  function guardSrc(proto, attr, replacement, label) {
    var d = Object.getOwnPropertyDescriptor(proto, attr);
    if (!d || !d.set) return;
    Object.defineProperty(proto, attr, {
      configurable: true, enumerable: d.enumerable,
      get: d.get,
      set: function (val) {
        var v = verdict(val);
        if (v) { report('R', label + ' → ' + v.host, pathOf(val) + ' → stub'); return d.set.call(this, replacement); }
        return d.set.call(this, val);
      }
    });
  }
  guardSrc(HTMLImageElement.prototype, 'src', GIF, 'pixel');
  guardSrc(HTMLScriptElement.prototype, 'src', 'data:text/javascript,', 'script');
  guardSrc(HTMLIFrameElement.prototype, 'src', 'about:blank', 'iframe');
  var SA = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, val) {
    if (typeof name === 'string' && name.toLowerCase() === 'src') {
      var v = verdict(val);
      if (v) {
        var tag = this.tagName;
        var rep = tag === 'IMG' ? GIF : tag === 'SCRIPT' ? 'data:text/javascript,' : tag === 'IFRAME' ? 'about:blank' : null;
        if (rep) { report('R', tag.toLowerCase() + ' → ' + v.host, pathOf(val) + ' → stub'); return SA.call(this, name, rep); }
      }
    }
    return SA.apply(this, arguments);
  };

  // --------------------------------------------------------------- WebSocket
  var WS = window.WebSocket;
  if (WS) {
    var FakeWS = function (url, protocols) {
      var v = verdict(url);
      if (!v) return protocols === undefined ? new WS(url) : new WS(url, protocols);
      report('R', 'ws → ' + v.host, 'socket refused');
      var t = new EventTarget();
      t.readyState = 3; t.url = String(url); t.send = function () {}; t.close = function () {};
      setTimeout(function () { var e = new Event('close'); t.dispatchEvent(e); if (t.onclose) t.onclose(e); }, 0);
      return t;
    };
    FakeWS.prototype = WS.prototype;
    ['CONNECTING', 'OPEN', 'CLOSING', 'CLOSED'].forEach(function (k) { FakeWS[k] = WS[k]; });
    window.WebSocket = FakeWS;
  }

  // ------------------------------------------------- analytics global stubs
  // Only installed while the name is unclaimed; a page's own value wins.
  var noop = function () {};
  var stub = new Proxy(noop, {
    get: function (t, k) { return k === 'then' || typeof k === 'symbol' ? undefined : k === 'push' ? function () { return 0; } : stub; },
    apply: function () { return stub; }
  });
  ['ga', 'fbq', '_fbq', 'mixpanel', 'amplitude', 'heap', 'hj', 'clarity', 'posthog', '_hsq', '_paq', '_gaq', 'ym', '_satellite', 'twq', 'ttq', 'pintrk', 'snaptr', 'rdt', 'lintrk', 'uetq'].forEach(function (name) {
    if (name in window) return;
    try {
      Object.defineProperty(window, name, {
        configurable: true, enumerable: false,
        get: function () { return stub; },
        set: function (v) { Object.defineProperty(window, name, { value: v, writable: true, configurable: true }); }
      });
    } catch (e) {}
  });

  // ------------------------------------------------------ privacy signals
  try { Object.defineProperty(Navigator.prototype, 'globalPrivacyControl', { get: function () { return true; }, configurable: true }); } catch (e) {}

  // ------------------------------------------- hyperlink auditing (<a ping>)
  document.addEventListener('click', function (e) {
    var a = e.target && e.target.closest && e.target.closest('a[ping]');
    if (a) { a.removeAttribute('ping'); report('B', location.hostname, 'a[ping] removed'); }
  }, true);

  // ----------------------------- keyboard fallback (engines without accel hooks)
  if (KEY_FALLBACK && window.top === window) {
    window.addEventListener('keydown', function (e) {
      var k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
      var combo = (e.ctrlKey ? 'ctrl+' : '') + (e.altKey ? 'alt+' : '') + (e.shiftKey ? 'shift+' : '') + k;
      if (/^(ctrl\+(shift\+)?[ltwbr1-9]|ctrl\+(shift\+)?Tab|alt\+(t|ArrowLeft|ArrowRight)|F5)$/.test(combo)) {
        e.preventDefault(); e.stopImmediatePropagation();
        try { window.ipc.postMessage(JSON.stringify({ key: combo })); } catch (x) {}
      }
    }, true);
  }
})();
