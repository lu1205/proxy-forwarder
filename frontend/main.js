const statusEl = document.querySelector("#serverStatus");
const forwardUrlEl = document.querySelector("#forwardUrl");
const healthUrlEl = document.querySelector("#healthUrl");
const requestBodyEl = document.querySelector("#requestBody");
const responseBodyEl = document.querySelector("#responseBody");
const sendTestEl = document.querySelector("#sendTest");

const invoke = window.__TAURI__?.core?.invoke;
const listen = window.__TAURI__?.event?.listen;

function setStatus(text, state = "ready") {
  statusEl.textContent = text;
  statusEl.className = `status-pill ${state}`;
}

async function copyValue(input) {
  await navigator.clipboard.writeText(input.value);
}

async function loadServiceInfo() {
  if (!invoke) {
    setStatus("浏览器预览", "ready");
    return;
  }

  const info = await invoke("get_service_info");
  forwardUrlEl.value = info.forwardUrl;
  healthUrlEl.value = info.healthUrl;
}

async function sendTestRequest() {
  responseBodyEl.textContent = "请求中...";

  try {
    const payload = JSON.parse(requestBodyEl.value);
    const response = await fetch(forwardUrlEl.value, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "x-request-id": crypto.randomUUID()
      },
      body: JSON.stringify(payload)
    });
    const data = await response.json();
    responseBodyEl.textContent = JSON.stringify(data, null, 2);
  } catch (error) {
    responseBodyEl.textContent = error instanceof Error ? error.message : String(error);
  }
}

document.querySelector("#copyForwardUrl").addEventListener("click", () => copyValue(forwardUrlEl));
document.querySelector("#copyHealthUrl").addEventListener("click", () => copyValue(healthUrlEl));
sendTestEl.addEventListener("click", sendTestRequest);

if (listen) {
  listen("forward-server-ready", (event) => {
    forwardUrlEl.value = event.payload.forwardUrl;
    healthUrlEl.value = event.payload.healthUrl;
    setStatus("服务已启动");
  });

  listen("forward-server-error", (event) => {
    setStatus("服务异常", "error");
    responseBodyEl.textContent = String(event.payload);
  });
}

loadServiceInfo().catch((error) => {
  setStatus("初始化异常", "error");
  responseBodyEl.textContent = error instanceof Error ? error.message : String(error);
});
