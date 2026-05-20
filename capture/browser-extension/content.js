let lastHash = "";

setInterval(async () => {
  const text = visibleText();
  if (text.length < 20) {
    return;
  }
  const hash = await sha256(text);
  if (hash === lastHash) {
    return;
  }
  lastHash = hash;

  chrome.runtime.sendMessage({
    type: "llm-attested-capture",
    href: window.location.href,
    subject: {
      provider: providerLabel(window.location.hostname),
      model: visibleModelLabel(),
      api_family: "browser-ui"
    },
    metrics: {
      visible_text_chars: text.length
    },
    hashes: {
      visible_transcript: hash
    },
    evidence: {
      page_title: document.title || "",
      capture_scope: "visible_text_hash"
    }
  });
}, 5000);

function visibleText() {
  return (document.body && document.body.innerText || "")
    .replace(/\s+/g, " ")
    .trim()
    .slice(-20000);
}

function visibleModelLabel() {
  const text = visibleText();
  const match = text.match(/\b(GPT[-\w. ]+|Claude[-\w. ]+|Gemini[-\w. ]+|Llama[-\w. ]+|Qwen[-\w. ]+)\b/i);
  return match ? match[0].slice(0, 80) : "unknown-browser-model";
}

function providerLabel(hostname) {
  if (hostname.includes("claude")) return "anthropic-web";
  if (hostname.includes("gemini")) return "gemini-web";
  if (hostname.includes("perplexity")) return "perplexity-web";
  if (hostname.includes("chatgpt")) return "openai-web";
  return "browser-llm";
}

async function sha256(value) {
  const bytes = new TextEncoder().encode(value);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return "sha256:" + [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}
