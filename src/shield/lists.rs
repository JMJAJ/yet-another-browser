//! Built-in rule data. Everything here is compiled into the binary; users can
//! extend or override it with `filters.txt` in the config directory.

/// Hosts whose every request is telemetry, analytics, ads or session replay.
/// Matching is by label suffix: `example.com` also blocks `a.b.example.com`.
pub const DOMAINS: &[&str] = &[
    // Google
    "google-analytics.com", "analytics.google.com", "googletagmanager.com",
    "googletagservices.com", "doubleclick.net", "googlesyndication.com",
    "googleadservices.com", "adservice.google.com", "app-measurement.com",
    "firebaselogging.googleapis.com", "firebaselogging-pa.googleapis.com",
    "crashlytics.com", "imasdk.googleapis.com",
    // Meta
    "connect.facebook.net", "pixel.facebook.com", "an.facebook.com",
    // Microsoft
    "clarity.ms", "bat.bing.com", "c.bing.com", "c.msn.com",
    "browser.events.data.microsoft.com", "mobile.events.data.microsoft.com",
    "self.events.data.microsoft.com", "vortex.data.microsoft.com",
    "telemetry.microsoft.com", "js.monitor.azure.com",
    "dc.services.visualstudio.com", "in.applicationinsights.azure.com",
    // Product analytics & session replay
    "hotjar.com", "hotjar.io", "mouseflow.com", "crazyegg.com", "fullstory.com",
    "lr-ingest.io", "lr-ingest.com", "lr-in.com", "lr-in-prod.com",
    "smartlook.com", "inspectlet.com", "luckyorange.com", "luckyorange.net",
    "quantummetric.com", "contentsquare.net", "clicktale.net", "decibelinsight.net",
    "glassboxdigital.io", "heapanalytics.com", "api.mixpanel.com", "api-js.mixpanel.com",
    "api-eu.mixpanel.com", "mxpnl.com", "api.amplitude.com", "api2.amplitude.com",
    "api.eu.amplitude.com", "cdn.amplitude.com", "cdn.segment.com", "api.segment.io",
    "cdn.segment.io", "api.rudderlabs.com", "cdn.rudderlabs.com", "i.posthog.com",
    "app.posthog.com", "matomo.cloud", "statcounter.com", "quantserve.com",
    "quantcount.com", "scorecardresearch.com", "chartbeat.com", "chartbeat.net",
    "nr-data.net", "js-agent.newrelic.com", "browser-intake-datadoghq.com",
    "browser-intake-datadoghq.eu", "pendo.io", "kissmetrics.io", "kissmetrics.com",
    "woopra.com", "getclicky.com", "visualwebsiteoptimizer.com", "logx.optimizely.com",
    "appboycdn.com", "app.adjust.com", "appsflyer.com", "api2.branch.io",
    "cloudflareinsights.com", "vercel-insights.com", "go-mpulse.net",
    "pipedream.wistia.com", "distillery.wistia.com", "siteintercept.qualtrics.com",
    // Error / crash telemetry
    "ingest.sentry.io", "ingest.us.sentry.io", "ingest.de.sentry.io",
    "browser.sentry-cdn.com", "sessions.bugsnag.com", "notify.bugsnag.com",
    "api.rollbar.com", "usage.trackjs.com", "capture.trackjs.com", "api.raygun.io",
    "api.honeybadger.io",
    // Marketing automation
    "hs-analytics.net", "track.hubspot.com", "hsadspixel.net", "munchkin.marketo.net",
    "mktoresp.com", "pi.pardot.com", "tiqcdn.com", "nexus.ensighten.com",
    "omtrdc.net", "demdex.net", "2o7.net", "everesttech.net", "adobedtm.com",
    // Social pixels
    "px.ads.linkedin.com", "snap.licdn.com", "analytics.twitter.com",
    "static.ads-twitter.com", "ads-api.twitter.com", "analytics.tiktok.com",
    "analytics-sg.tiktok.com", "ct.pinterest.com", "trk.pinterest.com",
    "log.pinterest.com", "tr.snapchat.com", "events.redditmedia.com",
    "alb.reddit.com", "q.quora.com", "mc.yandex.ru", "mc.yandex.com", "hm.baidu.com",
    "analytics.yahoo.com", "ads.yahoo.com",
    // Ad exchanges, data brokers, fingerprinting
    "criteo.com", "criteo.net", "taboola.com", "outbrain.com", "amazon-adsystem.com",
    "adnxs.com", "rubiconproject.com", "pubmatic.com", "openx.net", "casalemedia.com",
    "adsrvr.org", "moatads.com", "doubleverify.com", "adsafeprotected.com",
    "3lift.com", "sharethrough.com", "bidswitch.net", "smartadserver.com",
    "yieldmo.com", "teads.tv", "media.net", "zemanta.com", "bluekai.com", "krxd.net",
    "exelator.com", "rlcdn.com", "agkn.com", "tapad.com", "bounceexchange.com",
    "liadm.com", "id5-sync.com", "crwdcntrl.net", "mathtag.com", "adform.net",
    "serving-sys.com", "contextweb.com", "yieldlab.net", "33across.com", "gumgum.com",
    "onetag-sys.com", "sonobi.com", "indexww.com", "lijit.com", "sovrn.com",
    "emxdgt.com", "smaato.net", "fpjs.io", "fpcdn.io", "openfpcdn.io",
];

/// Telemetry that lives on a host which also serves real content:
/// (host suffix, path prefix).
pub const HOST_PATHS: &[(&str, &str)] = &[
    ("facebook.com", "/tr"),
    ("facebook.com", "/ajax/bz"),
    ("google.com", "/gen_204"),
    ("google.com", "/pagead"),
    ("play.google.com", "/log"),
    ("youtube.com", "/api/stats/"),
    ("youtube.com", "/ptracking"),
    ("youtube.com", "/pagead/"),
    ("plausible.io", "/api/event"),
    ("plausible.io", "/js/"),
    ("reddit.com", "/api/jail"),
];

/// Host-agnostic paths (query excluded) that are always telemetry.
/// Self-hosted analytics proxied through the first party land here.
pub const PATH_CONTAINS: &[&str] = &[
    "/g/collect", "/j/collect", "/r/collect", "/__utm.gif", "/matomo.php",
    "/piwik.php", "/_vercel/insights/", "/cdn-cgi/rum",
];

pub const PATH_SUFFIX: &[&str] = &[
    "/gtag/js", "/analytics.js", "/ga.js", "/fbevents.js", "/matomo.js", "/piwik.js",
    "/clarity.js", "/hotjar.js", "/beacon.min.js",
];

/// Query keys removed from every URL.
pub const PARAM_KEYS: &[&str] = &[
    "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid", "msclkid", "mc_cid",
    "mc_eid", "_ga", "_gl", "yclid", "ysclid", "igshid", "twclid", "ttclid",
    "li_fat_id", "s_cid", "vero_id", "vero_conv", "oly_anon_id", "oly_enc_id",
    "_hsenc", "_hsmi", "__hssc", "__hstc", "__hsfp", "hsctatracking", "mkt_tok",
    "wickedid", "rb_clickid", "irclickid", "_openstat", "srsltid", "epik", "sccid",
    "_kx", "ns_mchannel", "ns_source", "ns_campaign", "ns_linkname", "ns_fee", "__s",
    "ref_src", "xtor", "guccounter", "guce_referrer", "guce_referrer_sig", "soc_src",
    "soc_trk", "pk_campaign", "pk_kwd", "pk_source", "pk_medium", "pk_content",
    "pk_cid", "pk_vid",
];

/// Query key prefixes removed from every URL.
pub const PARAM_PREFIXES: &[&str] = &["utm_", "mtm_", "hsa_"];

/// Site-specific tracking keys: (host suffix, keys, prefixes).
pub const PARAM_SITE: &[(&str, &[&str], &[&str])] = &[
    ("youtube.com", &["si", "pp"], &[]),
    ("youtu.be", &["si"], &[]),
    ("open.spotify.com", &["si"], &[]),
    ("instagram.com", &["igsh"], &[]),
    ("x.com", &["s", "t"], &[]),
    ("twitter.com", &["s", "t"], &[]),
    ("linkedin.com", &["trk", "trackingid", "lipi"], &[]),
    ("amazon.", &["crid", "sprefix", "dib", "dib_tag"], &["pd_rd_", "pf_rd_"]),
];

/// Click-tracking redirect wrappers unwrapped on top-level navigation:
/// (host suffix, path, query key holding the real target).
pub const REDIRECTS: &[(&str, &str, &str)] = &[
    ("google.com", "/url", "q"),
    ("google.com", "/url", "url"),
    ("l.facebook.com", "/l.php", "u"),
    ("l.instagram.com", "/", "u"),
    ("l.messenger.com", "/l.php", "u"),
    ("youtube.com", "/redirect", "q"),
];
