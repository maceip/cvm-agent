const DEFAULT_DAEMON = "http://127.0.0.1:8090";

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!message || message.type !== "llm-attested-capture") {
    return false;
  }

  chrome.storage.sync.get(
    {
      daemonBaseUrl: DEFAULT_DAEMON,
      hackathonId: "hk_dev",
      teamId: "team_dev",
      agentSessionId: "browser_default"
    },
    async (settings) => {
      try {
        const report = {
          kind: "llm.call",
          hackathon_id: settings.hackathonId,
          team_id: settings.teamId,
          gateway_id: "browser_extension",
          manifest_hash: "",
          capture_method: "browser_extension",
          assurance: "participant_controlled",
          actor: {
            kind: "browser",
            agent_session_id: settings.agentSessionId
          },
          subject: message.subject || {},
          metrics: message.metrics || {},
          hashes: message.hashes || {},
          evidence: {
            ...(message.evidence || {}),
            browser_url_hash: await sha256(locationless(message.href || "")),
            browser_capture_version: "0.1.0"
          }
        };
        const resp = await fetch(`${settings.daemonBaseUrl}/capture/browser`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(report)
        });
        sendResponse({ ok: resp.ok, status: resp.status });
      } catch (error) {
        sendResponse({ ok: false, error: String(error) });
      }
    }
  );
  return true;
});

async function sha256(value) {
  const bytes = new TextEncoder().encode(value);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return "sha256:" + [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function locationless(href) {
  try {
    const url = new URL(href);
    return `${url.origin}${url.pathname}`;
  } catch {
    return href;
  }
}
