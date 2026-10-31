import { parseGuestEvent } from "../../src/protocol";

const start = document.getElementById("start");
const stop = document.getElementById("stop");
const input = document.getElementById("input");
const send = document.getElementById("send");
const script = document.getElementById("script");
const sendScript = document.getElementById("send-script");
const autoAck = document.getElementById("auto-ack");
const ack = document.getElementById("ack");
const status = document.getElementById("status");
const output = document.getElementById("output");
if (
  !(input instanceof HTMLInputElement) ||
  !(script instanceof HTMLTextAreaElement) ||
  !sendScript ||
  !(autoAck instanceof HTMLInputElement) ||
  !ack ||
  !start ||
  !stop ||
  !send ||
  !status ||
  !output
) {
  throw new Error("Missing harness controls");
}
const worker = new Worker(new URL("../../src/guest.worker.ts", import.meta.url), {
  type: "module",
});
worker.onmessage = (event: MessageEvent<unknown>) => {
  const message = parseGuestEvent(event.data);
  if (!message) throw new Error("Invalid worker response");
  if (message.type === "output") {
    output.textContent += new TextDecoder().decode(message.bytes);
    if (autoAck.checked) worker.postMessage({ type: "output-consumed" });
  } else {
    status.textContent = message.type === "error" ? `error: ${message.message}` : message.type;
  }
};
start.addEventListener("click", () => worker.postMessage({ type: "start" }));
stop.addEventListener("click", () => worker.postMessage({ type: "stop" }));
ack.addEventListener("click", () => worker.postMessage({ type: "output-consumed" }));
send.addEventListener("click", () =>
  worker.postMessage({
    type: "input",
    bytes: new TextEncoder().encode(input.value.replaceAll("\\n", "\n")),
  }),
);
sendScript.addEventListener("click", () =>
  worker.postMessage({ type: "input", bytes: new TextEncoder().encode(script.value) }),
);
