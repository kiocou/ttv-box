import { convertFileSrc } from "@tauri-apps/api/core";
import "./styles.css";
import runtimeUrl from "./app-runtime.js?url";

window.TtvConvertFileSrc = convertFileSrc;
window.TtvQrCode = {
  async toCanvas(...args) {
    const { default: QRCode } = await import("qrcode");
    return QRCode.toCanvas(...args);
  },
};
window.TtvLoadHls = async () => {
  if (!window.TtvHls) {
    const { default: Hls } = await import("hls.js");
    window.TtvHls = Hls;
  }
  return window.TtvHls;
};

window.TtvLoadDash = async () => {
  if (!window.TtvDash) {
    const { default: dashjs } = await import("dashjs");
    window.TtvDash = dashjs;
  }
  return window.TtvDash;
};

const runtime = document.createElement("script");
runtime.src = runtimeUrl;
runtime.async = false;
runtime.addEventListener("error", () => {
  document.body.dataset.runtimeError = "true";
  console.error("TTV application runtime failed to load");
});
document.body.appendChild(runtime);
