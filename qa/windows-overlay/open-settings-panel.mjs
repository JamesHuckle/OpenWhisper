const action = process.argv[2] === "close" ? "collapseOverlay" : "openMenu";
let target;
for (let attempt = 0; attempt < 50 && !target; attempt += 1) {
  const targets = await fetch("http://127.0.0.1:9222/json").then((response) => response.json());
  target = targets.find((candidate) => candidate.url?.includes("localhost:5173"));
  if (!target) await new Promise((resolve) => setTimeout(resolve, 100));
}
if (!target?.webSocketDebuggerUrl) {
  throw new Error("OpenWhisper WebView2 debug target was not found");
}

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

const response = new Promise((resolve, reject) => {
  socket.addEventListener(
    "message",
    (event) => {
      const payload = JSON.parse(event.data);
      if (payload.id !== 1) return;
      if (payload.error) reject(new Error(payload.error.message));
      else if (payload.result?.exceptionDetails) reject(new Error(payload.result.exceptionDetails.text));
      else resolve(payload);
    },
  );
});

socket.send(
  JSON.stringify({
    id: 1,
    method: "Runtime.evaluate",
    params: {
      expression: `(async () => {
        for (let attempt = 0; attempt < 50 && !window.__openWhisperTest; attempt += 1) {
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
        const menu = document.getElementById('mic-menu');
        const widget = document.getElementById('widget');
        if (window.__openWhisperTest) {
          await window.__openWhisperTest.${action}();
        } else {
          const dropdown = document.getElementById('btn-dropdown');
          const shouldBeOpen = '${action}' === 'openMenu';
          if (menu.classList.contains('hidden') === shouldBeOpen) dropdown.click();
          for (let attempt = 0; attempt < 50 && menu.classList.contains('hidden') === shouldBeOpen; attempt += 1) {
            await new Promise((resolve) => setTimeout(resolve, 20));
          }
        }
        return {
          menuHidden: menu.classList.contains('hidden'),
          menuOpen: widget.dataset.menuOpen,
          viewport: [innerWidth, innerHeight],
        };
      })()`,
      awaitPromise: true,
      returnByValue: true,
    },
  }),
);
const result = await response;
socket.close();
console.log(JSON.stringify(result.result.result.value));
