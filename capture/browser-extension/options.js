const fields = ["daemonBaseUrl", "hackathonId", "teamId", "agentSessionId"];

chrome.storage.sync.get({}, (settings) => {
  for (const field of fields) {
    if (settings[field]) {
      document.getElementById(field).value = settings[field];
    }
  }
});

document.getElementById("save").addEventListener("click", () => {
  const next = {};
  for (const field of fields) {
    next[field] = document.getElementById(field).value.trim();
  }
  chrome.storage.sync.set(next);
});
