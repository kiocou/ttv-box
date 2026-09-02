/* ================= 数据 ================= */
const CATALOG_IDS = [82, 169, 2993, 179, 118, 465];
const TVMAZE_API = 'https://api.tvmaze.com/shows/';
// Never present invented media as if it came from a connected source. The
// catalog is populated from TVMaze or the local/provider database at runtime.
let MOVIES = [];
// 深夜档防"全 0 闪现"：首次 applyCatalog 之前库数据还没到（18+ 条目分散在
// 分页的后段，首页前 200 条里通常一条都没有），此时渲染深夜档只会得到假空态。
let libraryDataReady = false;
let selectedMovie = MOVIES[0];
let detailMovie = selectedMovie;
let appMode = 'catalog';
const favoriteIds = new Set();
const CATALOG_FAVORITES_KEY = 'ttv.catalogFavorites';
let favoriteLoadGeneration = 0;
let libraryLoadGeneration = 0;
let librarySourceRenderSignature = '';
const POSTER_HYDRATION_LIMIT = 16;
const POSTER_HYDRATION_BATCH_SIZE = 2;
const POSTER_HYDRATION_TIMEOUT_MS = 8000;
const COVER_MAX_CONCURRENT = 6;
const COVER_LAZY_ROOT_MARGIN = '420px 0px';
const COVER_EAGER_COUNT = 8;
const COVER_PLACEHOLDER = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==';
// Toast is used by startup/hash routing as well as later UI actions. Define it
// before any async provider initialization can call a failure path.
const toastEl = document.getElementById('toast');
let toastTimer = null;

/* 桌面无边框窗口控制函数 (Window Controls) */
async function minimizeWindow(){
  try {
    if(window.__TAURI__?.window?.getCurrentWindow()){
      await window.__TAURI__.window.getCurrentWindow().minimize();
      return;
    }
    if(TtvBackend.available()){
      await TtvBackend.invoke('app_window_minimize');
      return;
    }
  } catch(error){
    toast('窗口最小化失败：' + backendErrorMessage(error));
    return;
  }
  toast('当前浏览器页面无法控制应用窗口。');
}
async function toggleMaximizeWindow(){
  try {
    if(window.__TAURI__?.window?.getCurrentWindow()){
      await window.__TAURI__.window.getCurrentWindow().toggleMaximize();
      return;
    }
    if(TtvBackend.available()){
      await TtvBackend.invoke('app_window_toggle_maximize');
      return;
    }
  } catch(error){
    toast('窗口大小切换失败：' + backendErrorMessage(error));
    return;
  }
  try{
    if(!document.fullscreenElement) await document.documentElement.requestFullscreen();
    else await document.exitFullscreen();
  }catch(error){ toast('当前页面无法切换窗口大小：' + backendErrorMessage(error)); }
}
async function closeWindow(){
  try {
    if(window.__TAURI__?.window?.getCurrentWindow()){
      await window.__TAURI__.window.getCurrentWindow().close();
      return;
    }
    if(TtvBackend.available()){
      await TtvBackend.invoke('app_window_close');
      return;
    }
  } catch(error){
    toast('关闭应用窗口失败：' + backendErrorMessage(error));
    return;
  }
  toast('当前浏览器页面无法关闭应用窗口。');
}

const TtvBackend = {
  available(){
    return typeof window !== 'undefined' && Boolean(window.__TAURI__?.core?.invoke || window.__TAURI_INTERNALS__?.invoke);
  },
  async invoke(command, args = {}){
    const invoke = window.__TAURI__?.core?.invoke || window.__TAURI_INTERNALS__?.invoke;
    if(!invoke) throw new Error('桌面端 IPC 不可用');
    return invoke(command, args);
  }
};

let SOURCE_CATALOG = [];
let activeCloudSource = null;
const SOURCE_IDS = {
  '本地磁盘': 'local', 'StreamHub': 'streamhub', 'OpenList': 'openlist',
  'WebDAV': 'webdav', 'SMB / NAS': 'smb', 'SFTP': 'sftp', '123云盘': 'cloud123',
  '百度网盘': 'baidu', '阿里云盘': 'aliyun', '夸克网盘': 'quark', '115网盘': '115',
  '天翼云盘': 'tianyi', '光鸭云盘': 'guangya'
};
 let guangyaOAuthPollTimer = null;
 let guangyaOAuthStatusTimer = null;
 let providerQrPollTimer = null;
async function loadSourceCatalog(){
  if(!TtvBackend.available()) return;
  try{ SOURCE_CATALOG = await TtvBackend.invoke('source_catalog') || []; }
  catch(error){ console.warn('Unable to read source catalog:', error); }
}
const CLOUD_ICONS = {
  openlist: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdPcGVuTGlzdCIgeDE9IjAlIiB5MT0iMCUiIHgyPSIxMDAlIiB5Mj0iMTAwJSI+CiAgICAgIDxzdG9wIG9mZnNldD0iMCUiIHN0b3AtY29sb3I9IiMwMjg0YzciLz4KICAgICAgPHN0b3Agb2Zmc2V0PSIxMDAlIiBzdG9wLWNvbG9yPSIjMGYxNzJhIi8+CiAgICA8L2xpbmVhckdyYWRpZW50PgogIDwvZGVmcz4KICA8cmVjdCB3aWR0aD0iMTI4IiBoZWlnaHQ9IjEyOCIgcng9IjI4IiBmaWxsPSJ1cmwoI2JnT3Blbkxpc3QpIi8+CiAgPGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoMjQsIDI0KSBzY2FsZSgzLjMzKSIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjMzhiZGY4IiBzdHJva2Utd2lkdGg9IjIuMiIgc3Ryb2tlLWxpbmVjYXA9InJvdW5kIiBzdHJva2UtbGluZWpvaW49InJvdW5kIj4KICAgIDxwYXRoIGQ9Ik0xMiAyTDIgMTkuNWgyMEwxMiAyeiIvPgogICAgPHBhdGggZD0iTTEyIDguNUw2IDE5LjVoMTJMMTIgOC41eiIvPgogIDwvZz4KPC9zdmc+',
  baidu: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdCYWlkdSIgeDE9IjAlIiB5MT0iMCUiIHgyPSIxMDAlIiB5Mj0iMTAwJSI+CiAgICAgIDxzdG9wIG9mZnNldD0iMCUiIHN0b3AtY29sb3I9IiMyNTYzZWIiLz4KICAgICAgPHN0b3Agb2Zmc2V0PSIxMDAlIiBzdG9wLWNvbG9yPSIjMWUzYThhIi8+CiAgICA8L2xpbmVhckdyYWRpZW50PgogIDwvZGVmcz4KICA8cmVjdCB3aWR0aD0iMTI4IiBoZWlnaHQ9IjEyOCIgcng9IjI4IiBmaWxsPSJ1cmwoI2JnQmFpZHUpIi8+CiAgPGVsbGlwc2UgY3g9IjQwIiBjeT0iNDYiIHJ4PSI4IiByeT0iMTEiIGZpbGw9IiNmZmZmZmYiLz4KICA8ZWxsaXBzZSBjeD0iNTYiIGN5PSIzNiIgcng9IjgiIHJ5PSIxMiIgZmlsbD0iI2ZmZmZmZiIvPgogIDxlbGxpcHNlIGN4PSI3MiIgY3k9IjM2IiByeD0iOCIgcnk9IjEyIiBmaWxsPSIjZmZmZmZmIi8+CiAgPGVsbGlwc2UgY3g9Ijg4IiBjeT0iNDYiIHJ4PSI4IiByeT0iMTEiIGZpbGw9IiNmZmZmZmYiLz4KICA8cGF0aCBkPSJNNDIgNzQgQzM4IDYwLCA1MCA1MiwgNjQgNTIgQzc4IDUyLCA5MCA2MCwgODYgNzQgQzgyIDg2LCA3NiA5MiwgNjQgOTIgQzUyIDkyLCA0NiA4NiwgNDIgNzQgWiIgZmlsbD0iI2ZmZmZmZiIvPgogIDxjaXJjbGUgY3g9IjY0IiBjeT0iNzIiIHI9IjcuNSIgZmlsbD0iI2VmNDQ0NCIvPgo8L3N2Zz4=',
  aliyun: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdBbGkiIHgxPSIwJSIgeTE9IjAlIiB4Mj0iMTAwJSIgeTI9IjEwMCUiPgogICAgICA8c3RvcCBvZmZzZXQ9IjAlIiBzdG9wLWNvbG9yPSIjZmY4YzAwIi8+CiAgICAgIDxzdG9wIG9mZnNldD0iMTAwJSIgc3RvcC1jb2xvcj0iI2VhNTgwYyIvPgogICAgPC9saW5lYXJHcmFkaWVudD4KICA8L2RlZnM+CiAgPHJlY3Qgd2lkdGg9IjEyOCIgaGVpZ2h0PSIxMjgiIHJ4PSIyOCIgZmlsbD0idXJsKCNiZ0FsaSkiLz4KICA8cmVjdCB4PSIyOCIgeT0iMzgiIHdpZHRoPSI3MiIgaGVpZ2h0PSI1MiIgcng9IjE4IiBmaWxsPSJub25lIiBzdHJva2U9IiNmZmZmZmYiIHN0cm9rZS13aWR0aD0iMTEiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIvPgogIDxjaXJjbGUgY3g9IjY0IiBjeT0iNjQiIHI9IjcuNSIgZmlsbD0iI2ZmZmZmZiIvPgo8L3N2Zz4=',
  cloud123: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmcxMjMiIHgxPSIwJSIgeTE9IjAlIiB4Mj0iMTAwJSIgeTI9IjEwMCUiPgogICAgICA8c3RvcCBvZmZzZXQ9IjAlIiBzdG9wLWNvbG9yPSIjM2I4MmY2Ii8+CiAgICAgIDxzdG9wIG9mZnNldD0iMTAwJSIgc3RvcC1jb2xvcj0iIzFkNGVkOCIvPgogICAgPC9saW5lYXJHcmFkaWVudD4KICA8L2RlZnM+CiAgPHJlY3Qgd2lkdGg9IjEyOCIgaGVpZ2h0PSIxMjgiIHJ4PSIyOCIgZmlsbD0idXJsKCNiZzEyMykiLz4KICA8dGV4dCB4PSI2NCIgeT0iODIiIGZvbnQtZmFtaWx5PSItYXBwbGUtc3lzdGVtLCBCbGlua01hY1N5c3RlbUZvbnQsICdTZWdvZSBVSScsIFJvYm90bywgc2Fucy1zZXJpZiIgZm9udC1zaXplPSI0NCIgZm9udC13ZWlnaHQ9IjkwMCIgZmlsbD0iI2ZmZmZmZiIgdGV4dC1hbmNob3I9Im1pZGRsZSIgbGV0dGVyLXNwYWNpbmc9Ii0xLjUiPjEyMzwvdGV4dD4KICA8cGF0aCBkPSJNMzggOTQgTDkwIDk0IiBzdHJva2U9IiM5M2M1ZmQiIHN0cm9rZS13aWR0aD0iNCIgc3Ryb2tlLWxpbmVjYXA9InJvdW5kIi8+Cjwvc3ZnPg==',
  quark: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdRdWFyayIgeDE9IjAlIiB5MT0iMCUiIHgyPSIxMDAlIiB5Mj0iMTAwJSI+CiAgICAgIDxzdG9wIG9mZnNldD0iMCUiIHN0b3AtY29sb3I9IiM0ZjQ2ZTUiLz4KICAgICAgPHN0b3Agb2Zmc2V0PSIxMDAlIiBzdG9wLWNvbG9yPSIjMzEyZTgxIi8+CiAgICA8L2xpbmVhckdyYWRpZW50PgogIDwvZGVmcz4KICA8cmVjdCB3aWR0aD0iMTI4IiBoZWlnaHQ9IjEyOCIgcng9IjI4IiBmaWxsPSJ1cmwoI2JnUXVhcmspIi8+CiAgPGNpcmNsZSBjeD0iNjQiIGN5PSI2NCIgcj0iMzMiIGZpbGw9Im5vbmUiIHN0cm9rZT0iIzM4YmRmOCIgc3Ryb2tlLXdpZHRoPSIxMCIvPgogIDxjaXJjbGUgY3g9IjY0IiBjeT0iNjQiIHI9IjEzIiBmaWxsPSIjZmZmZmZmIi8+CiAgPHBhdGggZD0iTTc4IDc4IEw5NCA5NCIgc3Ryb2tlPSIjMzhiZGY4IiBzdHJva2Utd2lkdGg9IjkiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIvPgo8L3N2Zz4=',
  p115: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmcxMTUiIHgxPSIwJSIgeTE9IjAlIiB4Mj0iMTAwJSIgeTI9IjEwMCUiPgogICAgICA8c3RvcCBvZmZzZXQ9IjAlIiBzdG9wLWNvbG9yPSIjMDI4NGM3Ii8+CiAgICAgIDxzdG9wIG9mZnNldD0iMTAwJSIgc3RvcC1jb2xvcj0iIzAzNjlhMSIvPgogICAgPC9saW5lYXJHcmFkaWVudD4KICA8L2RlZnM+CiAgPHJlY3Qgd2lkdGg9IjEyOCIgaGVpZ2h0PSIxMjgiIHJ4PSIyOCIgZmlsbD0idXJsKCNiZzEyMykiLz4KICA8dGV4dCB4PSI2NCIgeT0iODIiIGZvbnQtZmFtaWx5PSItYXBwbGUtc3lzdGVtLCBCbGlua01hY1N5c3RlbUZvbnQsICdTZWdvZSBVSScsIFJvYm90bywgc2Fucy1zZXJpZiIgZm9udC1zaXplPSI0NiIgZm9udC13ZWlnaHQ9IjkwMCIgZmlsbD0iI2ZmZmZmZiIgdGV4dC1hbmNob3I9Im1pZGRsZSIgbGV0dGVyLXNwYWNpbmc9Ii0yIj4xMTU8L3RleHQ+CiAgPGNpcmNsZSBjeD0iMTAyIiBjeT0iNDAiIHI9IjYiIGZpbGw9IiMzOGJkZjgiLz4KPC9zdmc+',
  tianyi: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdUaWFueWkiIHgxPSIwJSIgeTE9IjAlIiB4Mj0iMTAwJSIgeTI9IjEwMCUiPgogICAgICA8c3RvcCBvZmZzZXQ9IjAlIiBzdG9wLWNvbG9yPSIjMGVhNWU5Ii8+CiAgICAgIDxzdG9wIG9mZnNldD0iMTAwJSIgc3RvcC1jb2xvcj0iIzAyODRjNyIvPgogICAgPC9saW5lYXJHcmFkaWVudD4KICA8L2RlZnM+CiAgPHJlY3Qgd2lkdGg9IjEyOCIgaGVpZ2h0PSIxMjgiIHJ4PSIyOCIgZmlsbD0idXJsKCNiZ1RpYW55aSkiLz4KICA8cGF0aCBkPSJNNDQgODQgQzM1IDg0LCAyOCA3NywgMjggNjggQzI4IDYwLCAzNCA1MywgNDIgNTIgQzQ1IDM5LCA1NiAzMCwgNjkgMzAgQzgzIDMwLCA5NCA0MCwgOTYgNTQgQzEwMyA1NSwgMTA4IDYxLCAxMDggNjggQzEwOCA3NywgMTAxIDg0LCA5MiA4NCBaIiBmaWxsPSIjZmZmZmZmIi8+CiAgPGNpcmNsZSBjeD0iNjgiIGN5PSI1OCIgcj0iMTAiIGZpbGw9IiMwMjg0YzciIG9wYWNpdHk9IjAuMjUiLz4KPC9zdmc+',
  guangya: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdHdWFuZ3lhIiB4MT0iMCUiIHkxPSIwJSIgeDI9IjEwMCUiIHkyPSIxMDAlIj4KICAgICAgPHN0b3Agb2Zmc2V0PSIwJSIgc3RvcC1jb2xvcj0iI2Y5NzMxNiIvPgogICAgICA8c3RvcCBvZmZzZXQ9IjEwMCUiIHN0b3AtY29sb3I9IiNjMjQxMGMiLz4KICAgIDwvbGluZWFyR3JhZGllbnQ+CiAgPC9kZWZzPgogIDxyZWN0IHdpZHRoPSIxMjgiIGhlaWdodD0iMTI4IiByeD0iMjgiIGZpbGw9InVybCgjYmdHdWFuZ3lhKSIvPgogIDxjaXJjbGUgY3g9IjU2IiBjeT0iNDYiIHI9IjE4IiBmaWxsPSIjZmZmZmZmIi8+CiAgPHBhdGggZD0iTTM4IDY0IEMzOCA1MiwgNTQgNTIsIDY0IDUyIEM3OCA1MiwgOTggNjIsIDk4IDc2IEM5OCA4OCwgODIgOTIsIDYwIDkyIEM0MiA5MiwgMzggODAsIDM4IDY0IFoiIGZpbGw9IiNmZmZmZmYiLz4KICA8Y2lyY2xlIGN4PSI2MiIgY3k9IjQ0IiByPSIzLjUiIGZpbGw9IiNlYTU4MGMiLz4KICA8cGF0aCBkPSJNNjggNDYgTDg2IDQ4IEw3MCA1NCBaIiBmaWxsPSIjZmFjYzE1Ii8+Cjwvc3ZnPg==',
  webdav: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdXZWJEQVYiIHgxPSIwJSIgeTE9IjAlIiB4Mj0iMTAwJSIgeTI9IjEwMCUiPgogICAgICA8c3RvcCBvZmZzZXQ9IjAlIiBzdG9wLWNvbG9yPSIjMDM2OWExIi8+CiAgICAgIDxzdG9wIG9mZnNldD0iMTAwJSIgc3RvcC1jb2xvcj0iIzBjNGE2ZSIvPgogICAgPC9saW5lYXJHcmFkaWVudD4KICA8L2RlZnM+CiAgPHJlY3Qgd2lkdGg9IjEyOCIgaGVpZ2h0PSIxMjgiIHJ4PSIyOCIgZmlsbD0idXJsKCNiZ1dlYkRBVikiLz4KICA8ZyB0cmFuc2Zvcm09InRyYW5zbGF0ZSgyOCwgMjgpIHNjYWxlKDMpIiBmaWxsPSJub25lIiBzdHJva2U9IiM3ZGQzZmMiIHN0cm9rZS13aWR0aD0iMiIgc3Ryb2tlLWxpbmVjYXA9InJvdW5kIiBzdHJva2UtbGluZWpvaW49InJvdW5kIj4KICAgIDxyZWN0IHg9IjIiIHk9IjIiIHdpZHRoPSIyMCIgaGVpZ2h0PSI4IiByeD0iMiIgcnk9IjIiLz4KICAgIDxyZWN0IHg9IjIiIHk9IjE0IiB3aWR0aD0iMjAiIGhlaWdodD0iOCIgcng9IjIiIHJ5PSIyIi8+CiAgICA8bGluZSB4MT0iNiIgeTE9IjYiIHgyPSI2LjAxIiB5Mj0iNiIvPgogICAgPGxpbmUgeDE9IjYiIHkxPSIxOCIgeDI9IjYuMDEiIHkyPSIxOCIvPgogIDwvZz4KPC9zdmc+',
  nas: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdOQVMiIHgxPSIwJSIgeTE9IjAlIiB4Mj0iMTAwJSIgeTI9IjEwMCUiPgogICAgICA8c3RvcCBvZmZzZXQ9IjAlIiBzdG9wLWNvbG9yPSIjNDMzOGNhIi8+CiAgICAgIDxzdG9wIG9mZnNldD0iMTAwJSIgc3RvcC1jb2xvcj0iIzFlMWI0YiIvPgogICAgPC9saW5lYXJHcmFkaWVudD4KICA8L2RlZnM+CiAgPHJlY3Qgd2lkdGg9IjEyOCIgaGVpZ2h0PSIxMjgiIHJ4PSIyOCIgZmlsbD0idXJsKCNiZ05BUykiLz4KICA8ZyB0cmFuc2Zvcm09InRyYW5zbGF0ZSgyOCwgMjgpIHNjYWxlKDMpIiBmaWxsPSJub25lIiBzdHJva2U9IiNhNWI0ZmMiIHN0cm9rZS13aWR0aD0iMiIgc3Ryb2tlLWxpbmVjYXA9InJvdW5kIiBzdHJva2UtbGluZWpvaW49InJvdW5kIj4KICAgIDxyZWN0IHg9IjQiIHk9IjQiIHdpZHRoPSIxNiIgaGVpZ2h0PSIxNiIgcng9IjIiLz4KICAgIDxyZWN0IHg9IjkiIHk9IjkiIHdpZHRoPSI2IiBoZWlnaHQ9IjYiLz4KICAgIDxsaW5lIHgxPSI5IiB5MT0iMSIgeDI9IjkiIHkyPSI0Ii8+CiAgICA8bGluZSB4MT0iMTUiIHkxPSIxIiB4Mj0iMTUiIHkyPSI0Ii8+CiAgICA8bGluZSB4MT0iOSIgeTE9IjIwIiB4Mj0iOSIgeTI9IjIzIi8+CiAgICA8bGluZSB4MT0iMTUiIHkxPSIyMCIgeDI9IjE1IiB5Mj0iMjMiLz4KICAgIDxsaW5lIHgxPSIyMCIgeTE9IjkiIHgyPSIyMyIgeTI9IjkiLz4KICAgIDxsaW5lIHgxPSIyMCIgeTE9IjE0IiB4Mj0iMjMiIHkyPSIxNCIvPgogICAgPGxpbmUgeDE9IjEiIHkxPSI5IiB4Mj0iNCIgeTI9IjkiLz4KICAgIDxsaW5lIHgxPSIxIiB5MT0iMTQiIHgyPSI0IiB5Mj0iMTQiLz4KICA8L2c+Cjwvc3ZnPg==',
  sftp: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdTRlRQIiB4MT0iMCUiIHkxPSIwJSIgeDI9IjEwMCUiIHkyPSIxMDAlIj4KICAgICAgPHN0b3Agb2Zmc2V0PSIwJSIgc3RvcC1jb2xvcj0iIzdlMjJjZSIvPgogICAgICA8c3RvcCBvZmZzZXQ9IjEwMCUiIHN0b3AtY29sb3I9IiMzYjA3NjQiLz4KICAgIDwvbGluZWFyR3JhZGllbnQ+CiAgPC9kZWZzPgogIDxyZWN0IHdpZHRoPSIxMjgiIGhlaWdodD0iMTI4IiByeD0iMjgiIGZpbGw9InVybCgjYmdTRlRQKSIvPgogIDxnIHRyYW5zZm9ybT0idHJhbnNsYXRlKDI4LCAyOCkgc2NhbGUoMykiIGZpbGw9Im5vbmUiIHN0cm9rZT0iI2Q4YjRmZSIgc3Ryb2tlLXdpZHRoPSIyIiBzdHJva2UtbGluZWNhcD0icm91bmQiIHN0cm9rZS1saW5lam9pbj0icm91bmQiPgogICAgPHJlY3QgeD0iMyIgeT0iMTEiIHdpZHRoPSIxOCIgaGVpZ2h0PSIxMSIgcng9IjIiIHJ5PSIyIi8+CiAgICA8cGF0aCBkPSJNNyAxMVY3YTUgNSAwIDAgMSAxMCAwdjQiLz4KICAgIDxjaXJjbGUgY3g9IjEyIiBjeT0iMTYiIHI9IjEuNSIgZmlsbD0iI2Q4YjRmZSIvPgogIDwvZz4KPC9zdmc+',
  local: 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4IiB3aWR0aD0iMTAwJSIgaGVpZ2h0PSIxMDAlIj4KICA8ZGVmcz4KICAgIDxsaW5lYXJHcmFkaWVudCBpZD0iYmdMb2NhbCIgeDE9IjAlIiB5MT0iMCUiIHgyPSIxMDAlIiB5Mj0iMTAwJSI+CiAgICAgIDxzdG9wIG9mZnNldD0iMCUiIHN0b3AtY29sb3I9IiMwNTk2NjkiLz4KICAgICAgPHN0b3Agb2Zmc2V0PSIxMDAlIiBzdG9wLWNvbG9yPSIjMDY0ZTNiIi8+CiAgICA8L2xpbmVhckdyYWRpZW50PgogIDwvZGVmcz4KICA8cmVjdCB3aWR0aD0iMTI4IiBoZWlnaHQ9IjEyOCIgcng9IjI4IiBmaWxsPSJ1cmwoI2JnTG9jYWwpIi8+CiAgPGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoMjgsIDI4KSBzY2FsZSgzKSIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjNmVlN2I3IiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI+CiAgICA8bGluZSB4MT0iMjIiIHkxPSIxMiIgeDI9IjIiIHkyPSIxMiIvPgogICAgPHBhdGggZD0iTTUuNDUgNS4xMUwyIDEydjZhMiAyIDAgMCAwIDIgMmgxNmEyIDIgMCAwIDAgMi0ydi02bC0zLjQ1LTYuODlBMiAyIDAgMCAwIDE2Ljc2IDRINy4yNGEyIDIgMCAwIDAtMS43OSAxLjExeiIvPgogICAgPGxpbmUgeDE9IjYiIHkxPSIxNiIgeDI9IjYuMDEiIHkyPSIxNiIvPgogICAgPGxpbmUgeDE9IjEwIiB5MT0iMTYiIHgyPSIxMC4wMSIgeTI9IjE2Ii8+CiAgPC9nPgo8L3N2Zz4='
};

// Keep every cloud/provider surface on the same bundled, official artwork.
Object.assign(CLOUD_ICONS, {
  openlist: 'assets/cloud-providers/openlist.svg',
  baidu: 'assets/cloud-providers/baidu.svg',
  aliyun: 'assets/cloud-providers/aliyun.svg',
  cloud123: 'assets/cloud-providers/123pan.ico',
  quark: 'assets/cloud-providers/quark.ico',
  p115: 'assets/cloud-providers/115.ico',
  tianyi: 'assets/cloud-providers/tianyi.ico',
  guangya: 'assets/cloud-providers/guangya.png',
  webdav: 'assets/cloud-providers/webdav.svg',
  nas: 'assets/cloud-providers/nas.svg',
  sftp: 'assets/cloud-providers/sftp.svg',
  local: 'assets/cloud-providers/local.svg'
});

function normalizeCloudProviderIcons(){
  const artwork = {'百度网盘': CLOUD_ICONS.baidu, '阿里云盘': CLOUD_ICONS.aliyun};
  document.querySelectorAll('#view-cloud .svc-card').forEach(card => {
    const name = card.querySelector('.svc-name')?.textContent?.trim();
    const src = artwork[name];
    if(!src) return;
    const image = card.querySelector('.svc-icon img');
    if(image){ image.src = src; image.removeAttribute('srcset'); }
  });
}
function sourceVisual(name){
  const visuals = {
    'OpenList': {html: `<img src="${CLOUD_ICONS.openlist}" alt="OpenList">`, color: '#38bdf8'},
    '百度网盘': {html: `<img src="${CLOUD_ICONS.baidu}" alt="百度网盘">`, color: '#3b82f6'},
    '阿里云盘': {html: `<img src="${CLOUD_ICONS.aliyun}" alt="阿里云盘">`, color: '#ff6a00'},
    'WebDAV': {html: `<img src="${CLOUD_ICONS.webdav}" alt="WebDAV">`, color: '#38bdf8'},
    'SMB / NAS': {html: `<img src="${CLOUD_ICONS.nas}" alt="SMB / NAS">`, color: '#818cf8'},
    'SFTP': {html: `<img src="${CLOUD_ICONS.sftp}" alt="SFTP">`, color: '#c084fc'},
    '本地磁盘': {html: `<img src="${CLOUD_ICONS.local}" alt="本地磁盘">`, color: '#34d399'},
    '123云盘': {html: `<img src="${CLOUD_ICONS.cloud123}" alt="123云盘">`, color: '#3b82f6'},
    '夸克网盘': {html: `<img src="${CLOUD_ICONS.quark}" alt="夸克网盘">`, color: '#6366f1'},
    '115网盘': {html: `<img src="${CLOUD_ICONS.p115}" alt="115网盘">`, color: '#2563eb'},
    '天翼云盘': {html: `<img src="${CLOUD_ICONS.tianyi}" alt="天翼云盘">`, color: '#0284c7'},
    '光鸭云盘': {html: `<img src="${CLOUD_ICONS.guangya}" alt="光鸭云盘">`, color: '#ea580c'},
    'StreamHub': {html: 'SH', color: '#2dd4bf'}
  };
  return visuals[name] || {html: '云', color: '#94a3b8'};
}

const openlistStorageCache = new Map();
const openlistSourceStorage = new Map();
function isOpenListSource(source){
  return Boolean(source?.id) && source.id !== 'guangya' && source.id !== 'local' && source.id !== 'streamhub';
}
function openlistDriverForSource(source){
  const drivers = {
    openlist: 'OpenList',
    webdav: 'WebDAV',
    smb: 'SMB',
    sftp: 'SFTP',
    cloud123: '123Pan',
    baidu: 'BaiduNetdisk',
    aliyun: 'AliyunDrive',
    quark: 'Quark',
    '115': '115',
    tianyi: '189Cloud'
  };
  return drivers[source?.id] || source?.name || 'OpenList';
}
function openlistStorageForSource(source){
  if(!source) return null;
  const cachedId = openlistSourceStorage.get(source.id);
  const items = Array.from(openlistStorageCache.values());
  return (cachedId && openlistStorageCache.get(cachedId))
    || items.find(item => item.id === cachedId)
    || items.find(item => item.name === source.name)
    || items.find(item => String(item.driver || '').toLowerCase() === openlistDriverForSource(source).toLowerCase());
}
function renderOpenListServiceStatus(status){
  const pill = document.getElementById('qrStatusPill');
  if(!pill) return;
  const reachable = Boolean(status?.reachable);
  pill.innerHTML = `<span class="dot" style="background:${reachable ? '#10b981' : '#f59e0b'}"></span> ${reachable ? 'OpenList 服务在线' : 'OpenList 服务未连接'}`;
}
async function loadOpenListStatus(){
  if(!TtvBackend.available()) return null;
  try{
    const status = await TtvBackend.invoke('openlist_status');
    renderOpenListServiceStatus(status);
    return status;
  }catch(error){
    renderOpenListServiceStatus({reachable:false});
    return null;
  }
}
async function loadOpenListStorages(){
  if(!TtvBackend.available()) return [];
  const items = await TtvBackend.invoke('openlist_storage_list');
  openlistStorageCache.clear();
  (Array.isArray(items) ? items : []).forEach(item => openlistStorageCache.set(String(item.id), item));
  return Array.from(openlistStorageCache.values());
}
function renderOpenListStorageForm(source, storage, schema){
  const body = document.getElementById('cloudSourceBody');
  const action = document.getElementById('cloudSourceAction');
  if(!body) return;
  const fields = Array.isArray(schema?.fields) && schema.fields.length ? schema.fields : [
    {key:'username',label:'账号 / Client ID',kind:'text',secret:false,required:false},
    {key:'password',label:'密码 / Token',kind:'password',secret:true,required:false}
  ];
  body.innerHTML = `<div class="openlist-config">
    <div class="openlist-service-line"><span>OpenList 服务</span><strong id="openlistInlineStatus">检查中…</strong></div>
    <label class="openlist-field"><span>挂载名称</span><input id="openlistStorageName" value="${escapeHtml(storage?.name || source.name)}" autocomplete="off"></label>
    <label class="openlist-field"><span>挂载路径</span><input id="openlistMountPath" value="${escapeHtml(storage?.mountPath || '/') }" autocomplete="off"></label>
    ${fields.map(field => {
      const type = field.secret || field.kind === 'password' ? 'password' : (field.kind === 'number' ? 'number' : 'text');
      const current = storage?.fields?.[field.key] || '';
      return `<label class="openlist-field"><span>${escapeHtml(field.label || field.key)}${field.required ? ' *' : ''}</span><input data-openlist-field="${escapeHtml(field.key)}" type="${type}" value="${escapeHtml(current)}" autocomplete="off"></label>`;
    }).join('')}
    <div class="openlist-config-hint">凭据仅发送到本地 OpenList 服务，不会写入前端存储。</div>
  </div>`;
  TtvBackend.invoke('openlist_status').then(status => {
    const inline = document.getElementById('openlistInlineStatus');
    if(inline) inline.textContent = status?.reachable ? `在线 · ${status.version || '本地服务'}` : '未连接';
  }).catch(() => {});
  if(action){
    action.hidden = false;
    action.textContent = storage ? '保存并测试' : '保存并挂载';
    action.onclick = () => saveOpenListStorage(source);
  }
}
async function configureOpenListSource(source){
  if(!TtvBackend.available()) return toast('OpenList 配置需要在 TTV 桌面端运行。');
  try{
    const status = await loadOpenListStatus();
    if(!status?.reachable){
      const body = document.getElementById('cloudSourceBody');
      if(body) body.innerHTML = `<div style="display:flex;flex-direction:column;gap:10px;color:var(--text-dim);font-size:12px;line-height:1.7"><strong style="color:var(--danger)">OpenList 服务未连接</strong><span>${escapeHtml(status?.runtime?.message || '请配置 TTV_OPENLIST_BIN，或将 OpenList 放入 resources/openlist。')}</span><button class="btn btn-accent" id="openlistRestartInline">重新启动 OpenList</button></div>`;
      document.getElementById('openlistRestartInline')?.addEventListener('click', async () => { await TtvBackend.invoke('openlist_start').catch(() => {}); configureOpenListSource(source); });
      return;
    }
    if(status.authenticated === false){
      const body = document.getElementById('cloudSourceBody');
      if(body) body.innerHTML = `<div class="openlist-config">
        <div class="openlist-service-line"><span>OpenList 服务</span><strong style="color:#6ee7b7">在线 · 需要登录</strong></div>
        <label class="openlist-field"><span>OpenList 管理员账号</span><input id="openlistAdminUsername" value="admin" autocomplete="username"></label>
        <label class="openlist-field"><span>OpenList 管理员密码</span><input id="openlistAdminPassword" type="password" value="admin" autocomplete="current-password"></label>
        <div class="openlist-config-hint">默认填入 admin / admin；登录一次后，当前网盘会显示独立的 OpenList 存储配置项。</div>
        <button class="btn btn-accent" id="openlistLoginSubmit" type="button">登录 OpenList</button>
      </div>`;
      document.getElementById('openlistLoginSubmit')?.addEventListener('click', loginOpenList);
      return;
    }
    const storage = openlistStorageForSource(source);
    const schema = await TtvBackend.invoke('openlist_storage_schema', {driver: openlistDriverForSource(source)}).catch(() => null);
    renderOpenListStorageForm(source, storage, schema);
  }catch(error){
    toast('读取 OpenList 配置失败：' + backendErrorMessage(error));
  }
}
async function loginOpenList(){
  const username = document.getElementById('openlistAdminUsername')?.value.trim();
  const password = document.getElementById('openlistAdminPassword')?.value || '';
  if(!username || !password) return toast('请输入 OpenList 管理员账号和密码。');
  const button = document.getElementById('openlistLoginSubmit');
  if(button){ button.disabled = true; button.textContent = '登录中…'; }
  try{
    await TtvBackend.invoke('openlist_login', {input:{username, password}});
    toast('OpenList 登录成功。');
    if(activeCloudSource) configureOpenListSource(activeCloudSource);
  }catch(error){
    toast('OpenList 登录失败：' + backendErrorMessage(error));
    if(button){ button.disabled = false; button.textContent = '登录 OpenList'; }
  }
}
async function saveOpenListStorage(source){
  try{
    const storage = openlistStorageForSource(source);
    const fields = {};
    document.querySelectorAll('[data-openlist-field]').forEach(input => {
      if(input.value.trim()) fields[input.dataset.openlistField] = input.value;
    });
    const input = {
      id: storage?.id || null,
      name: document.getElementById('openlistStorageName')?.value.trim() || source.name,
      driver: storage?.driver || openlistDriverForSource(source),
      mountPath: document.getElementById('openlistMountPath')?.value.trim() || '/',
      enabled: true,
      fields
    };
    const saved = await TtvBackend.invoke('openlist_storage_save', {input});
    if(saved?.id){ openlistStorageCache.set(String(saved.id), saved); openlistSourceStorage.set(source.id, String(saved.id)); }
    let tested = saved;
    if(saved?.id){
      try{
        tested = await TtvBackend.invoke('openlist_storage_test', {id: saved.id});
      }catch(error){
        if(saved?.id) openlistStorageCache.set(String(saved.id), saved);
        toast(`${source.name} 配置已保存，但 OpenList 存储测试失败：${backendErrorMessage(error)}`);
        selectCloudSource(source.name);
        return;
      }
    }
    if(tested?.id) openlistStorageCache.set(String(tested.id), tested);
    toast(`${source.name} 已通过 OpenList 保存${tested?.connection === 'connected' ? '并连接' : ''}。`);
    selectCloudSource(source.name);
  }catch(error){ toast('保存 OpenList 存储失败：' + backendErrorMessage(error)); }
}
function selectCloudSource(name){
  const id = SOURCE_IDS[name];
  const source = SOURCE_CATALOG.find(item => item.id === id) || {name, id, protocol: 'source', implemented: false, browseFiles: false, playbackResolution: false};
  if(activeCloudSource?.id && activeCloudSource.id !== source.id) resetCloudBrowserState(source.id);
  activeCloudSource = source;
  document.querySelectorAll('#view-cloud .svc-card').forEach(card => {
    const handler = card.getAttribute('onclick') || '';
    card.classList.toggle('cloud-selected', handler.includes(`'${name}'`));
  });
  document.getElementById('currentDriveBread').textContent = source.name;
  const visual = sourceVisual(source.name);
  const logo = document.getElementById('cloudSourceLogo');
  const title = document.getElementById('cloudSourceTitle');
  const tip = document.getElementById('cloudSourceTip');
  const status = document.getElementById('qrStatusPill');
  const body = document.getElementById('cloudSourceBody');
  const action = document.getElementById('cloudSourceAction');
  const scan = document.getElementById('cloudScanAction');
  const logout = document.getElementById('cloudLogoutAction');
  if(logo){ logo.innerHTML = visual.html; logo.style.borderColor = visual.color + '66'; logo.style.color = visual.color; }
  if(title) title.textContent = source.name;
  if(status) status.innerHTML = '<span class="dot"></span> ' + (source.implemented ? '已接入适配器' : '等待协议配置');
  if(tip) tip.textContent = isOpenListSource(source) ? `OPENLIST · ${source.browseFiles ? '统一文件浏览与播放' : '等待挂载'}` : (source.implemented ? `${source.protocol.toUpperCase()} · ${source.browseFiles ? '可浏览文件' : '尚未开放文件浏览'}` : `需要配置可验证的 ${source.protocol.toUpperCase()} 适配器`);
  if(body) body.innerHTML = `
    <div class="guangya-status-lines" style="text-align:left">
      <div class="guangya-status-item">
        <span>认证协议</span>
        <b>${escapeHtml(source.protocol.toUpperCase())}</b>
      </div>
      <div class="guangya-status-item">
        <span>接入模式</span>
        <b>${escapeHtml(source.loginMode || '标准 OAuth 2.0')}</b>
      </div>
      <div class="guangya-status-item full-width">
        <span>服务状态</span>
        <b style="color:${source.implemented ? '#3ddc84' : 'var(--text-faint)'}">${source.implemented ? (source.browseFiles ? '✓ 已接入真实目录读取与推流' : '✓ 已接入认证框架') : '• 协议适配器就绪中'}</b>
      </div>
    </div>`;
  if(action){
    const cloudLoginSource = ['baidu', 'aliyun', 'cloud123', 'quark', '115', 'tianyi', 'guangya'].includes(source.id);
    action.textContent = isOpenListSource(source) ? '配置 OpenList 存储' : (source.id === 'local' ? '选择并扫描目录' : (source.id === 'streamhub' ? '读取 StreamHub 媒体库' : (cloudLoginSource ? '二维码登录' : '配置 ' + source.name)));
    action.onclick = () => {
      if(source.id === 'local') return startScanPipeline();
      if(source.id === 'streamhub') return loadStreamHubResources();
      if(isOpenListSource(source)) return configureOpenListSource(source);
      if(cloudLoginSource) return startCloudScanLogin(source);
      openDriveModal(source.name);
    };
  }
  const libraryAction = document.getElementById('cloudLibraryAction');
  if(libraryAction){
    const browserCapable = canOpenCloudBrowser(source);
    libraryAction.textContent = browserCapable ? '浏览云盘文件' : '查看媒体库';
    libraryAction.onclick = () => browserCapable ? openSelectedCloudLibrary() : showView('library');
  }
  if(scan){
    scan.textContent = isOpenListSource(source) ? '扫描 OpenList 目录' : (source.id === 'local' ? '扫描本地目录' : (source.browseFiles || source.id === 'guangya' ? '扫描目录 · 刮削' : '查看媒体库'));
    scan.onclick = () => {
      if(source.id === 'local') return startScanPipeline();
      if(canOpenCloudBrowser(source)) return openGuangyaFileBrowser();
      return showView('library');
    };
  }
  if(logout) logout.hidden = true;
  if(source.id === 'guangya') loadGuangyaOAuthStatus();
  else if(isOpenListSource(source)) configureOpenListSource(source);
  else if(['baidu', 'aliyun'].includes(source.id)) loadProviderOAuthStatus(source);
}
function filterCloudGrid(tabBtn, category){
  if(tabBtn){
    tabBtn.parentElement.querySelectorAll('.cloud-tab').forEach(b => b.classList.remove('active'));
    tabBtn.classList.add('active');
  }
  document.querySelectorAll('#view-cloud .svc-card').forEach(card => {
    const cardCat = card.dataset.category || 'all';
    if(category === 'all' || cardCat === category){
      card.style.display = 'flex';
    } else {
      card.style.display = 'none';
    }
  });
}
function escapeHtml(value){
  return String(value ?? '').replace(/[&<>"']/g, char => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
}
function formatOAuthExpiry(expiresAt){
  if(!expiresAt) return '未提供到期时间';
  return new Date(Number(expiresAt) * 1000).toLocaleString('zh-CN', {hour12:false});
}
function clearGuangyaOAuthPoll(){
  if(guangyaOAuthPollTimer){ window.clearTimeout(guangyaOAuthPollTimer); guangyaOAuthPollTimer = null; }
}
function clearGuangyaOAuthStatusTimer(){
  if(guangyaOAuthStatusTimer){ window.clearTimeout(guangyaOAuthStatusTimer); guangyaOAuthStatusTimer = null; }
}
function scheduleGuangyaOAuthStatusRefresh(status){
  clearGuangyaOAuthStatusTimer();
  if(!status?.sessionRefresh || !status.refreshAvailable || status.connection !== 'connected') return;
  const nowSeconds = Math.floor(Date.now() / 1000);
  const refreshBoundarySeconds = status.expiresAt
    ? Number(status.expiresAt) - 600
    : nowSeconds + 60;
  const delayMs = Math.max(1000, (refreshBoundarySeconds - nowSeconds) * 1000);
  guangyaOAuthStatusTimer = window.setTimeout(() => {
    void loadGuangyaOAuthStatus();
  }, delayMs);
}
function clearProviderQrPoll(){
  if(providerQrPollTimer){ window.clearTimeout(providerQrPollTimer); providerQrPollTimer = null; }
}
async function loadProviderCapabilities(providerId){
  if(!TtvBackend.available() || !providerId) return null;
  try{
    return await TtvBackend.invoke('provider_capabilities', {providerId});
  }catch(error){
    console.warn('Unable to read provider capabilities:', providerId, error);
    return null;
  }
}
async function loadGuangyaOAuthStatus(){
  const body = document.getElementById('cloudSourceBody');
  const action = document.getElementById('cloudSourceAction');
  const logout = document.getElementById('cloudLogoutAction');
  if(!TtvBackend.available()) return;
  clearGuangyaOAuthStatusTimer();
  try{
    const status = await TtvBackend.invoke('guangya_oauth_status');
    const connected = status.connection === 'connected';
    const expired = status.connection === 'expired';
    if(logout) logout.hidden = !connected;
    if(action){
      action.textContent = connected ? '重新扫码登录' : '扫码登录';
      action.onclick = () => status.configured ? startGuangyaDeviceOAuth() : loadGuangyaOAuthStatus();
    }
    if(!status.configured){
      if(body) body.innerHTML = `<div style="display:flex;flex-direction:column;gap:10px;text-align:left;font-size:12px;line-height:1.7"><div style="color:var(--text-faint)">光鸭扫码登录尚未就绪，缺少：<code>${status.missingFields.map(escapeHtml).join('</code>、<code>')}</code></div><div style="color:var(--text-faint)">当前逆向记录未确认光鸭官方二维码和设备码参数，因此应用不会构造或模拟登录二维码。</div><div style="color:var(--text-faint)">配置位置：<code>${escapeHtml(status.configFile)}</code></div></div>`;
      return;
    }
    const stateLabel = connected ? '已连接' : (expired ? '授权已过期' : '未连接');
    const stateColor = connected ? '#3ddc84' : (expired ? '#ff9f43' : 'var(--text-dim)');
    const expiresLabel = status.expiresAt ? formatOAuthExpiry(status.expiresAt) : '服务端未返回';
    if(body) body.innerHTML = `
      <div class="guangya-login-card">
        <div class="guangya-login-mark">
          <img src="${CLOUD_ICONS.guangya}" alt="光鸭云盘">
          <span>✓</span>
        </div>
        <strong>光鸭云盘 · 已登录</strong>
        <small>${escapeHtml(status.accountId ? '账户 ' + status.accountId : '已恢复云端授权令牌')}</small>
      </div>
      <div class="guangya-status-lines">
        <div class="guangya-status-item">
          <span>OAuth 状态</span>
          <strong style="color:${stateColor}">${stateLabel}</strong>
        </div>
        <div class="guangya-status-item">
          <span>访问令牌到期</span>
          <b>${expiresLabel}</b>
        </div>
        <div class="guangya-status-item">
          <span>扫码授权</span>
          <b>${status.deviceCodeLogin ? '可随时启动' : '未开启'}</b>
        </div>
        <div class="guangya-status-item">
          <span>令牌刷新</span>
          <b>${status.sessionRefresh ? (status.refreshAvailable ? '自动续期' : '需重新扫码') : '手动'}</b>
        </div>
        <div class="guangya-status-item">
          <span>文件读取</span>
          <b>${status.browseFiles ? '真实目录支持' : '仅基础流'}</b>
        </div>
      </div>
      ${expired ? '<div style="margin-top:10px;color:var(--text-faint);font-size:12px;line-height:1.6">访问令牌已过期且刷新未完成，请重新扫码授权后再读取目录。</div>' : (status.sessionRefresh && status.refreshAvailable ? '<div style="margin-top:10px;color:var(--text-faint);font-size:12px;line-height:1.6">自动续期已开启，将在到期前静默换新令牌。</div>' : '')}`;
    scheduleGuangyaOAuthStatusRefresh(status);
  }catch(error){
    if(body) body.innerHTML = `<p style="color:var(--danger);font-size:12px;line-height:1.7">无法读取光鸭 OAuth 状态：${escapeHtml(backendErrorMessage(error))}</p>`;
  }
}

async function loadProviderOAuthStatus(source){
  const body = document.getElementById('cloudSourceBody');
  const action = document.getElementById('cloudSourceAction');
  const logout = document.getElementById('cloudLogoutAction');
  if(!TtvBackend.available() || !source?.id) return;
  try{
    const status = await TtvBackend.invoke('provider_session_status', {providerId:source.id});
    const connected = status.connection === 'connected';
    const expired = status.connection === 'expired';
    if(logout) logout.hidden = !connected;
    if(action){
      action.textContent = connected ? '重新二维码授权' : '二维码授权登录';
      action.onclick = () => startCloudScanLogin(source);
    }
    const visual = sourceVisual(source.name);
    const stateLabel = connected ? '已连接' : (expired ? '授权已过期' : '未连接');
    const stateColor = connected ? '#3ddc84' : (expired ? '#ff9f43' : 'var(--text-dim)');
    if(body) body.innerHTML = `
      <div class="guangya-login-card">
        <div class="guangya-login-mark" style="border-color:${visual.color}55">
          ${visual.html}
          <span>${connected ? '✓' : '·'}</span>
        </div>
        <strong>${escapeHtml(source.name)} · ${stateLabel}</strong>
        <small>${connected ? escapeHtml(status.accountId ? '账户 ' + status.accountId : '官方 OAuth 令牌已安全恢复') : '二维码将在当前面板内显示，不会打开新窗口'}</small>
      </div>
      <div class="guangya-status-lines">
        <div class="guangya-status-item"><span>授权状态</span><strong style="color:${stateColor}">${stateLabel}</strong></div>
        <div class="guangya-status-item"><span>自动轮询</span><b>${status.capabilities?.deviceCodeLogin ? '官方设备码' : '授权码回填'}</b></div>
        <div class="guangya-status-item"><span>文件读取</span><b>${status.capabilities?.browseFiles ? '官方开放 API' : '未开放'}</b></div>
        <div class="guangya-status-item"><span>视频解析</span><b>${status.capabilities?.playbackResolution ? '官方下载地址' : '未开放'}</b></div>
      </div>
      ${expired ? '<div style="margin-top:10px;color:var(--text-faint);font-size:12px;line-height:1.6">授权已过期，请重新完成二维码授权。</div>' : ''}`;
  }catch(error){
    if(body) body.innerHTML = `<p style="color:var(--danger);font-size:12px;line-height:1.7">无法读取 ${escapeHtml(source.name)} 授权状态：${escapeHtml(backendErrorMessage(error))}</p>`;
  }
}
async function openSelectedCloudLibrary(){
  // Resolve from the current selection, then fall back to the browser source.
  // Some older source descriptors omitted browseFiles even though the adapter
  // is browser-capable; provider IDs are authoritative for those adapters.
  const source = activeCloudSource
    || SOURCE_CATALOG.find(item => item.id === cloudBrowserProviderId)
    || null;
  if(canOpenCloudBrowser(source)){
    if(source && !activeCloudSource) activeCloudSource = source;
    await openGuangyaFileBrowser();
    return;
  }
  showView('library');
}

function canOpenCloudBrowser(source){
  return Boolean(source?.browseFiles || source?.id === 'guangya' || isOpenListSource(source));
}
let cloudBrowserProviderId = 'guangya';
let guangyaBrowser = {parentId:null, folderName:'根目录', stack:[], entries:[], nextPageToken:null};
const guangyaSelectedFolders = new Map();
const guangyaImportingFolders = new Set();
function activeBrowserSource(){
  return activeCloudSource || SOURCE_CATALOG.find(item => item.id === cloudBrowserProviderId) || {id:cloudBrowserProviderId, name:'云盘'};
}
function resetCloudBrowserState(providerId = activeCloudSource?.id || 'guangya'){
  cloudBrowserProviderId = providerId;
  guangyaBrowser = {parentId:null, folderName:'根目录', stack:[], entries:[], nextPageToken:null};
  guangyaSelectedFolders.clear();
  guangyaImportingFolders.clear();
}
const GUANGYA_VIDEO_EXTENSIONS = ['mp4','mkv','webm','avi','mov','m4v','ts','m2ts','flv','wmv','rm','rmvb','3gp','mpeg','mpg','vob','ogv'];
function isGuangyaVideo(item){
  const mime = String(item?.mimeType || '').toLowerCase();
  if(mime.startsWith('video/')) return true;
  const name = String(item?.name || '').toLowerCase();
  return GUANGYA_VIDEO_EXTENSIONS.some(ext => name.endsWith('.' + ext)) || String(item?.metadata?.mediaType || '').toLowerCase().includes('video');
}
function guangyaFileKind(item){
  if(item?.kind === 'folder') return 'folder';
  const mime = String(item?.mimeType || '').toLowerCase();
  const name = String(item?.name || '').toLowerCase();
  if(mime.startsWith('video/') || isGuangyaVideo(item)) return 'video';
  if(mime.startsWith('audio/') || /\.(mp3|flac|wav|m4a|aac|ogg|opus|ape|wma)$/.test(name)) return 'audio';
  if(mime.startsWith('image/') || /\.(jpg|jpeg|png|gif|webp|bmp|svg|heic|avif)$/.test(name)) return 'image';
  if(/\.(zip|rar|7z|tar|gz|bz2|xz|iso)$/.test(name)) return 'archive';
  if(mime.startsWith('text/') || /\.(txt|md|pdf|doc|docx|xls|xlsx|ppt|pptx|srt|ass|ssa|csv|json|xml)$/.test(name)) return 'document';
  return 'file';
}
function guangyaFileIcon(item){
  const kind = guangyaFileKind(item);
  return {folder:'📁', video:'🎬', audio:'♫', image:'🖼', archive:'🗜', document:'📄', file:'📦'}[kind];
}
function guangyaBrowserPath(){
  return ['根目录', ...guangyaBrowser.stack.map(item => item.name), guangyaBrowser.folderName].filter((name, index, list) => index === 0 || name !== list[index - 1]);
}
function renderGuangyaBrowser(){
  const body = document.getElementById('cloudSourceBody');
  if(!body || currentView === 'cloud' || currentView === 'cloud-browser'){ renderGuangyaMiniFiles(); renderCloudBrowser(); return; }
  const folders = guangyaBrowser.entries.filter(item => item.kind === 'folder');
  const files = guangyaBrowser.entries.filter(item => item.kind !== 'folder');
  body.innerHTML = `<div style="display:flex;flex-direction:column;gap:10px;text-align:left;font-size:12px;line-height:1.55">
    <div style="color:var(--text-faint)">当前位置：${guangyaBrowserPath().map(escapeHtml).join(' / ')}</div>
    <div style="display:flex;gap:8px;flex-wrap:wrap">
      ${guangyaBrowser.stack.length ? '<button class="btn btn-ghost" id="guangyaBackFolder">返回上级</button>' : ''}
      ${guangyaBrowser.parentId !== null && guangyaBrowser.parentId !== undefined && String(guangyaBrowser.parentId) !== '' ? '<button class="btn btn-accent" id="guangyaAddCurrentFolder">添加此文件夹视频</button>' : ''}
      ${guangyaBrowser.nextPageToken ? '<button class="btn btn-ghost" id="guangyaLoadMore">加载更多</button>' : ''}
    </div>
    <div style="color:var(--text-faint)">文件夹 ${folders.length} 个 · 视频 ${files.filter(isGuangyaVideo).length} 个 · 其他文件 ${files.filter(item => !isGuangyaVideo(item)).length} 个 · 已加入扫描配置 ${guangyaSelectedFolders.size} 个</div>
      ${guangyaSelectedFolders.size ? '<button class="btn btn-accent" id="guangyaScanSelected">开始扫描已加入目录 · 刮削</button>' : ''}
    <div id="guangyaFolderList" style="display:flex;flex-direction:column;gap:6px">
      ${folders.length ? folders.map((item, index) => `<div data-folder-index="${index}" style="display:flex;align-items:center;gap:8px;padding:9px 10px;border:1px solid var(--line);border-radius:7px;background:rgba(255,255,255,.025)"><label style="display:flex;align-items:center;gap:8px;min-width:0;flex:1"><input type="checkbox" class="guangya-folder-select" data-folder-index="${index}" ${guangyaSelectedFolders.has(item.id) ? 'checked' : ''}><span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">文件夹 · ${escapeHtml(item.name)}</span></label><button class="btn btn-ghost guangya-folder-open" data-folder-index="${index}">打开</button></div>`).join('') : '<div style="color:var(--text-faint)">当前目录没有子文件夹。</div>'}
    </div>
    <div style="display:flex;flex-direction:column;gap:5px">
      ${files.length ? files.map(item => `<div style="display:flex;align-items:center;justify-content:space-between;gap:8px;padding:8px 10px;border-bottom:1px solid var(--line);color:${isGuangyaVideo(item) ? 'var(--text)' : 'var(--text-faint)'}"><span>${isGuangyaVideo(item) ? '视频' : '文件'} · ${escapeHtml(item.name)}</span><span>${isGuangyaVideo(item) ? '可添加' : '非视频，已过滤'}</span></div>`).join('') : '<div style="color:var(--text-faint)">当前目录没有文件。</div>'}
    </div>
  </div>`;
  document.getElementById('guangyaBackFolder')?.addEventListener('click', () => {
    const parent = guangyaBrowser.stack[guangyaBrowser.stack.length - 1] || {id:null, name:'根目录'};
    guangyaBrowser.stack = guangyaBrowser.stack.slice(0, -1);
    loadGuangyaResources(parent.id, parent.name, false);
  });
  document.getElementById('guangyaLoadMore')?.addEventListener('click', () => loadGuangyaResources(guangyaBrowser.parentId, guangyaBrowser.folderName, true));
  document.getElementById('guangyaAddCurrentFolder')?.addEventListener('click', () => addGuangyaFolder(guangyaBrowser.parentId, guangyaBrowser.folderName));
  document.getElementById('guangyaScanSelected')?.addEventListener('click', scanSelectedGuangyaFolders);
  document.querySelectorAll('.guangya-folder-select').forEach(input => input.addEventListener('change', () => {
    const item = folders[Number(input.dataset.folderIndex)];
    if(!item?.id) return;
    if(input.checked){
      guangyaSelectedFolders.set(item.id, item.name);
      updateCloudScanSelectionState(`${item.name} 已加入扫描配置。`);
    }else{
      guangyaSelectedFolders.delete(item.id);
      updateCloudScanSelectionState();
      toast('已取消勾选；已导入的影视不会被删除。');
    }
  }));
  document.querySelectorAll('.guangya-folder-open').forEach(button => button.addEventListener('click', () => {
    const item = folders[Number(button.dataset.folderIndex)];
    guangyaBrowser.stack.push({id:guangyaBrowser.parentId, name:guangyaBrowser.folderName});
    loadGuangyaResources(item.id, item.name, false);
  }));
  document.querySelectorAll('.guangya-folder-add').forEach(button => button.addEventListener('click', () => {
    const item = folders[Number(button.dataset.folderIndex)];
    addGuangyaFolder(item.id, item.name);
  }));
  renderGuangyaMiniFiles();
  renderCloudBrowser();
}
function renderGuangyaMiniFiles(){
  const target = document.getElementById('cloudMiniFileList');
  if(!target) return;
  const entries = Array.isArray(guangyaBrowser.entries) ? guangyaBrowser.entries : [];
  const path = guangyaBrowserPath();
  const directory = document.getElementById('cloudRenameDirectory');
  if(directory) directory.value = path.length > 1 ? '/' + path.slice(1).join('/') : '/';
  target.innerHTML = `<div class="cloud-mini-files-head"><span>云盘文件 · ${entries.length} 项</span><span>${guangyaBrowser.stack.length ? '<button type="button" class="cloud-mini-back" id="cloudMiniBack">返回上级</button>' : escapeHtml(path.join(' / '))}</span></div>${entries.length ? entries.map((item, index) => {
    const folder = item.kind === 'folder';
    const kind = guangyaFileKind(item);
    const content = `<span class="cloud-mini-file-content"><span class="cloud-mini-file-icon ${kind}" title="${kind}">${guangyaFileIcon(item)}</span><span title="${escapeHtml(item.name)}">${escapeHtml(item.name)}</span></span><span class="cloud-mini-type">${folder ? '进入 ›' : escapeHtml(formatBytes(item.sizeBytes))}</span>`;
    return folder
      ? `<div class="cloud-mini-file"><button type="button" class="cloud-mini-folder" data-mini-folder-index="${index}" aria-label="打开文件夹 ${escapeHtml(item.name)}">${content}</button></div>`
      : `<div class="cloud-mini-file">${content}</div>`;
  }).join('') : '<div class="cloud-mini-file"><span>当前目录没有文件。</span></div>'}${guangyaBrowser.nextPageToken ? '<div class="cloud-mini-file"><button type="button" class="btn btn-ghost" id="cloudMiniLoadMore">加载更多</button></div>' : ''}`;
  target.querySelectorAll('.cloud-mini-folder').forEach(button => button.addEventListener('click', () => {
    const item = entries[Number(button.dataset.miniFolderIndex)];
    if(!item?.id) return toast('该文件夹缺少有效目录 ID。');
    guangyaBrowser.stack.push({id:guangyaBrowser.parentId, name:guangyaBrowser.folderName});
    loadGuangyaResources(item.id, item.name, false);
  }));
  target.querySelector('#cloudMiniBack')?.addEventListener('click', () => {
    const parent = guangyaBrowser.stack[guangyaBrowser.stack.length - 1] || {id:null, name:'根目录'};
    guangyaBrowser.stack = guangyaBrowser.stack.slice(0, -1);
    loadGuangyaResources(parent.id, parent.name, false);
  });
  target.querySelector('#cloudMiniLoadMore')?.addEventListener('click', () => loadGuangyaResources(guangyaBrowser.parentId, guangyaBrowser.folderName, true));
}

function renderCloudBrowser(){
  const grid = document.getElementById('cloudBrowserGrid');
  if(!grid) return;
  const source = activeBrowserSource();
  const entries = Array.isArray(guangyaBrowser.entries) ? guangyaBrowser.entries : [];
  const path = document.getElementById('cloudBrowserPath');
  const count = document.getElementById('cloudBrowserCount');
  const back = document.getElementById('cloudBrowserBack');
  const subtitle = document.getElementById('cloudBrowserSubtitle');
  const icon = document.getElementById('cloudBrowserHeroIcon');
  const sideIcon = document.getElementById('cloudBrowserDriveIcon');
  if(path) path.textContent = '/' + guangyaBrowserPath().slice(1).join('/') + (guangyaBrowserPath().length > 1 ? '/' : '');
  if(count) count.textContent = `${entries.length} 项`;
  if(back) back.disabled = !guangyaBrowser.stack.length;
  if(subtitle) subtitle.textContent = `${guangyaBrowserPath().join(' / ')} · ${entries.filter(item => item.kind === 'folder').length} 个文件夹`;
  const iconKey = source.id === '115' ? 'p115' : source.id;
  const iconSource = CLOUD_ICONS[iconKey] || source.iconAsset || CLOUD_ICONS.guangya;
  if(icon) icon.src = iconSource;
  if(sideIcon) sideIcon.src = iconSource;
  const title = document.getElementById('cloudBrowserTitle');
  const driveName = document.querySelector('#cloudBrowserGuangyaDrive .cloud-browser-drive-info b');
  renderOpenListBrowserDrives();
  if(title) title.textContent = source.name || '云盘文件';
  if(driveName) driveName.textContent = source.name || '云盘';
  if(!entries.length){ grid.innerHTML = '<div class="cloud-browser-empty">当前目录没有可显示的真实文件。</div>'; return; }
  grid.innerHTML = entries.map((item, index) => {
    const folder = item.kind === 'folder';
    const video = !folder && isGuangyaVideo(item);
    const kind = folder ? 'folder' : (video ? 'video' : 'file');
    const iconSymbol = folder ? '📁' : (video ? '🎬' : '📄');
    return `
      <div class="cloud-file-card glass" data-browser-index="${index}" role="button" tabindex="0" aria-label="${escapeHtml(item.name)}">
        <div class="cloud-file-icon-wrap ${kind}">
          <span>${iconSymbol}</span>
        </div>
        <div class="cloud-file-info">
          <div class="cloud-file-name" title="${escapeHtml(item.name)}">${escapeHtml(item.name)}</div>
          <div class="cloud-file-meta">${folder ? '文件夹' : escapeHtml(formatBytes(item.sizeBytes))}</div>
        </div>
        ${folder ? `<input class="cloud-browser-folder-check" type="checkbox" data-folder-id="${escapeHtml(item.id)}" ${guangyaSelectedFolders.has(item.id) ? 'checked' : ''} aria-label="将 ${escapeHtml(item.name)} 加入扫描配置" title="加入扫描配置，确认后才会导入">` : ''}
        <span class="cloud-file-arrow">${folder ? '›' : (video ? '▶' : '')}</span>
      </div>`;
  }).join('');
  grid.querySelectorAll('[data-browser-index]').forEach(card => card.addEventListener('click', () => {
    const item = entries[Number(card.dataset.browserIndex)];
    if(!item) return;
    if(item.kind === 'folder'){
      guangyaBrowser.stack.push({id: guangyaBrowser.parentId, name: guangyaBrowser.folderName});
      loadGuangyaResources(item.id, item.name, false);
    }else if(isGuangyaVideo(item)){
      const storage = isOpenListSource(source) ? openlistStorageForSource(source) : null;
      openPlayer({id:`${isOpenListSource(source) ? 'openlist' : 'provider'}:${source.id}:` + item.id, providerId:isOpenListSource(source) ? 'openlist' : source.id, providerMediaId:isOpenListSource(source) ? (item.path || item.id) : item.id, openlistStorageId:storage?.id, openlistPath:item.path || item.id, t:item.name, type:'video', q:'VIDEO', img:item.thumbnailUrl || '/assets/detail-poster.jpg', homePoster:item.thumbnailUrl || '', d:formatDuration(item.durationSeconds), y:'—', r:0, summary:`来自${source.name}的真实视频文件。`, sourceLabel:source.name});
    }else{
      toast('该文件不是可播放的视频。');
    }
  }));
  grid.querySelectorAll('.cloud-browser-folder-check').forEach(input => input.addEventListener('click', event => event.stopPropagation()));
  grid.querySelectorAll('.cloud-browser-folder-check').forEach(input => input.addEventListener('change', () => {
    const item = entries.find(entry => String(entry.id) === String(input.dataset.folderId));
    if(!item) return;
    if(input.checked){ guangyaSelectedFolders.set(item.id, item.name); updateCloudScanSelectionState(`${item.name} 已加入扫描配置。`); }
    else { guangyaSelectedFolders.delete(item.id); updateCloudScanSelectionState(); toast('已取消勾选；已导入的影视不会被删除。'); }
  }));
}
function renderOpenListBrowserDrives(){
  const target = document.getElementById('openlistBrowserDrives');
  if(!target) return;
  const storages = Array.from(openlistStorageCache.values()).filter(item => item.enabled !== false);
  target.innerHTML = storages.map(storage => {
    const active = String(openlistStorageForSource(activeBrowserSource())?.id || '') === String(storage.id);
    return `<button class="cloud-browser-drive${active ? ' active' : ''}" data-openlist-storage="${escapeHtml(storage.id)}">
      <div class="cloud-browser-drive-icon">☁</div>
      <div class="cloud-browser-drive-info"><b>${escapeHtml(storage.name || storage.driver || 'OpenList 存储')}</b><small>${escapeHtml(storage.driver || 'OpenList')} · ${storage.connection === 'connected' ? '已连接' : '待测试'}</small></div>
      <span class="drive-chevron">›</span>
    </button>`;
  }).join('');
  target.querySelectorAll('[data-openlist-storage]').forEach(button => button.addEventListener('click', async () => {
    const storage = openlistStorageCache.get(String(button.dataset.openlistStorage));
    if(!storage) return;
    openlistStorageCache.set(String(storage.id), storage);
    openlistSourceStorage.set(activeCloudSource?.id || 'openlist', String(storage.id));
    if(activeCloudSource?.id !== 'openlist'){
      activeCloudSource = {id:'openlist', name:storage.name || 'OpenList 存储', protocol:'openlist', browseFiles:true, implemented:true};
    }
    resetCloudBrowserState('openlist');
    showView('cloud-browser');
    await loadOpenListResources('/', '根目录', false);
  }));
}

function formatBytes(value){
  const size = Number(value || 0);
  if(!size) return '文件';
  if(size < 1024 * 1024) return `${Math.round(size / 1024)} KB`;
  if(size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

async function openGuangyaFileBrowser(){
  const source = activeCloudSource
    || SOURCE_CATALOG.find(item => item.id === cloudBrowserProviderId)
    || null;
  if(!canOpenCloudBrowser(source)) return toast('该来源尚未提供可验证的文件接口。');
  if(!TtvBackend.available()) return toast(`${source.name}文件浏览需要在桌面端运行。`);
  if(isOpenListSource(source)){
    try{
      await loadOpenListStatus();
      const storages = await loadOpenListStorages();
      const storage = openlistStorageForSource(source);
      if(!storage || !storages.length){
        toast(`请先为${source.name}配置 OpenList 存储。`);
        showView('cloud');
        return configureOpenListSource(source);
      }
      openlistSourceStorage.set(source.id, String(storage.id));
      if(cloudBrowserProviderId !== source.id) resetCloudBrowserState(source.id);
      showView('cloud-browser');
      await loadGuangyaResources(guangyaBrowser.parentId || '/', guangyaBrowser.folderName || '根目录', false);
    }catch(error){
      toast(`无法读取${source.name} OpenList 目录：${backendErrorMessage(error)}`);
    }
    return;
  }
  try{
    const status = await TtvBackend.invoke('provider_session_status', {providerId:source.id});
    if(status.connection !== 'connected'){
      toast(`请先完成${source.name}二维码授权。`);
      showView('cloud');
      selectCloudSource(source.name);
      return;
    }
  }catch(error){
    toast(`无法确认${source.name}登录状态：` + backendErrorMessage(error));
    return;
  }
  if(cloudBrowserProviderId !== source.id) resetCloudBrowserState(source.id);
  showView('cloud-browser');
  await loadGuangyaResources(guangyaBrowser.parentId, guangyaBrowser.folderName, false);
}
function closeGuangyaFileBrowser(){ showView('cloud'); }
function loadGuangyaRoot(){ guangyaBrowser.stack = []; loadGuangyaResources(null, '根目录', false); }
function goGuangyaBack(){
  if(!guangyaBrowser.stack.length) return;
  const parent = guangyaBrowser.stack[guangyaBrowser.stack.length - 1] || {id:null, name:'根目录'};
  guangyaBrowser.stack = guangyaBrowser.stack.slice(0, -1);
  loadGuangyaResources(parent.id, parent.name, false);
}
async function refreshGuangyaFiles(){ await loadGuangyaResources(guangyaBrowser.parentId, guangyaBrowser.folderName, false); }
function searchCurrentGuangyaFiles(){
  const query = window.prompt('搜索当前目录中的文件名', '');
  if(query == null) return;
  const text = query.trim().toLowerCase();
  if(!text) return renderGuangyaBrowser();
  const original = guangyaBrowser.entries;
  guangyaBrowser.entries = original.filter(item => String(item.name || '').toLowerCase().includes(text));
  renderGuangyaBrowser();
  guangyaBrowser.entries = original;
}
function updateCloudScanSelectionState(message){
  const subtitle = document.getElementById('cloudBrowserSubtitle');
  const folders = Array.isArray(guangyaBrowser.entries) ? guangyaBrowser.entries.filter(item => item.kind === 'folder').length : 0;
  if(subtitle) subtitle.textContent = `${guangyaBrowserPath().join(' / ')} · ${folders} 个文件夹 · 已加入扫描配置 ${guangyaSelectedFolders.size} 个`;
  if(message) toast(message);
}

function chooseAdultMarkForImport(folderName){
  return new Promise(resolve => {
    openModal(
      `18+ (NSFW) 标记 · ${folderName || '所选目录'}`,
      `
        <p style="color:var(--text-dim);font-size:12px;line-height:1.6;margin:0 0 12px">请选择这批视频的入库方式。自动判断会识别文件名中的强番号和 18+ 标记。</p>
        <div class="modal-field">
          <label>18+ (NSFW) 标记</label>
          <select class="modal-input" id="guangyaAddMarkAdult">
            <option value="auto" selected>自动判断（按文件名识别）</option>
            <option value="adult">整批标记为 18+（NSFW）</option>
            <option value="normal">不强行标为 18+（番号命中仍会隔离）</option>
          </select>
        </div>
      `,
      `
        <button class="btn btn-ghost" onclick="closeModal()">取消</button>
        <button class="btn btn-accent" id="guangyaAddMarkConfirm">开始导入</button>
      `
    );
    const confirm = document.getElementById('guangyaAddMarkConfirm');
    confirm?.addEventListener('click', () => {
      const choice = document.getElementById('guangyaAddMarkAdult')?.value || 'auto';
      closeModal();
      resolve(choice === 'adult' ? true : (choice === 'normal' ? false : null));
    });
    appModal?.querySelector('.modal-close')?.addEventListener('click', () => resolve(undefined), {once:true});
    appModal?.addEventListener('click', event => {
      if(event.target === appModal) resolve(undefined);
    }, {once:true});
  });
}

async function scanSelectedGuangyaFolders(){
  const source = activeBrowserSource();
  if(!guangyaSelectedFolders.size) return toast('请先勾选要加入扫描配置的文件夹。');
  if(isOpenListSource(source)){
    const selected = Array.from(guangyaSelectedFolders, ([path, name]) => ({path, name}));
    guangyaSelectedFolders.clear();
    renderGuangyaBrowser();
    for(const item of selected) await addOpenListFolder(item.path, item.name);
    return;
  }
  openModal(
    '扫描已加入目录 · 刮削',
    `
      <p style="color:var(--text-dim);font-size:12px;line-height:1.6;margin:0 0 10px">共 ${guangyaSelectedFolders.size} 个云盘目录将加入本次扫描并刮削。</p>
      <div class="modal-field">
        <label>18+ (NSFW) 标记</label>
        <select class="modal-input" id="guangyaScanMarkAdult">
          <option value="auto" selected>自动判断（按文件名识别）</option>
          <option value="adult">整批标记为 18+（NSFW）</option>
          <option value="normal">不强行标为 18+（番号命中仍会隔离）</option>
        </select>
      </div>
    `,
    `
      <button class="btn btn-ghost" onclick="closeModal()">取消</button>
      <button class="btn btn-accent" onclick="runSelectedGuangyaScan()">开始扫描并刮削</button>
    `
  );
}
async function runSelectedGuangyaScan(){
  const source = activeBrowserSource();
  const markAdultChoice = document.getElementById('guangyaScanMarkAdult')?.value || 'auto';
  const markAdult = markAdultChoice === 'adult' ? true : (markAdultChoice === 'normal' ? false : null);
  if(!guangyaSelectedFolders.size) return toast('请先勾选要加入扫描配置的文件夹。');
  const entries = Array.from(guangyaSelectedFolders);
  closeModal();
  await runCloudFoldersScanTask(source, entries, markAdult);
}
// The whole cloud scan job: walk the selected folders (import via upsert, so
// re-runs are idempotent), then scrape the unscraped backlog in batches. The
// folder list + provider are recorded on the task so an interrupted run can be
// retried with one click instead of re-picking folders.
async function runCloudFoldersScanTask(source, folderEntries, markAdult){
  if(!resetScanProgress('cloud', '扫描已加入目录 · 刮削')) return;
  scanProgress.providerId = source?.id || '';
  scanProgress.selectedFolders = folderEntries.map(([id, name]) => ({id:String(id), name:String(name)}));
  saveScanTasks();
  logScanProgress(`共 ${folderEntries.length} 个云盘目录加入本次扫描。`);
  try{
    const totals = {folders:0, files:0, imported:0, skipped:0, promotional:0, nonVideo:0};
    for(const [folderId, folderName] of folderEntries){
      updateScanProgress(`正在读取 ${folderName}`, null);
      logScanProgress(`开始递归扫描 ${source?.name || '云盘'} · ${folderName}`);
      const report = await TtvBackend.invoke('provider_sync_library_recursive', {
        providerId: source?.id,
        input:{rootId:String(folderId), pageSize:100, maxItems:100000, ...(markAdult === null ? {} : {markAdult})}
      });
      totals.folders += Number(report?.folders || 0);
      totals.files += Number(report?.fetched || 0);
      totals.imported += Number(report?.imported || 0);
      totals.skipped += Number(report?.skipped || 0);
      totals.promotional += Number(report?.skippedPromotional || 0);
      totals.nonVideo += Number(report?.skippedNonVideo || 0);
      Object.assign(scanProgress, totals);
      updateScanProgress(`${folderName} 已导入，继续处理下一项`, Math.round((totals.imported + totals.skipped) / Math.max(1, totals.files) * 100));
      logScanProgress(`${folderName}：发现 ${report?.fetched || 0} 项，导入 ${report?.imported || 0} 个视频`);
    }
    // 用户在导入阶段点了暂停：不要继续进入刮削（刮削命令会重置后端停止标志，
    // 等于把暂停吞掉后再跑几个小时），直接结束，已导入的数据保留。
    if(scanTaskPausedByUser()) return;
    scanProgress.folders = totals.folders;
    scanProgress.files = totals.files;
    scanProgress.imported = totals.imported;
    scanProgress.skipped = totals.skipped;
    scanProgress.promotional = totals.promotional;
    scanProgress.nonVideo = totals.nonVideo;
    updateScanProgress('云盘导入完成，快速源刮削（豆瓣/JavBus）', null);
    // 两阶段刮削：先用快而准的源把大部分内容刮掉（每条几秒），剩余未命中的
    // 再交给慢速源（JavDB/Avmoo/JavLibrary/Jav321）补刮，避免每条未匹配项
    // 都串行走完六个源。
    let scraped = await scrapeLibraryUntilDone(5000, 3, 'fast');
    if(scanTaskPausedByUser()) return;
    if(scraped && Number(scraped.updated || 0) >= 0){
      updateScanProgress('快速源完成，慢速源补刮剩余未匹配条目', null);
      logScanProgress('快速源阶段完成，剩余未匹配条目转交慢速源（JavDB/Avmoo/JavLibrary/Jav321）补刮。');
      const slow = await scrapeLibraryUntilDone(5000, 3, 'full');
      if(scanTaskPausedByUser()) return;
      scraped = {
        requested: Number(scraped?.requested || 0) + Number(slow?.requested || 0),
        updated: Number(scraped?.updated || 0) + Number(slow?.updated || 0),
        matched: Number(scraped?.matched || 0) + Number(slow?.matched || 0),
        unmatched: Number(scraped?.unmatched || 0) + Number(slow?.unmatched || 0),
        covers: Number(scraped?.covers || 0) + Number(slow?.covers || 0),
        adultIsolated: Number(scraped?.adultIsolated || 0) + Number(slow?.adultIsolated || 0),
        providers: slow?.providers || scraped?.providers,
      };
    }
    scanProgress.updated = Number(scraped?.updated || 0);
    updateScanProgress('刮削完成，刷新影视库', null);
    await refreshLibraryAfterImport();
    finishScanProgress(`${totals.folders} 个目录已完成：发现 ${totals.files} 项，导入或更新 ${totals.imported} 个视频${scrapeSummary(scraped)}。剩余未匹配条目可随时点「刮削缺失项」继续补刮。`);
    const warned = await maybeWarnTmdbMissing(scraped);
    if(!warned) toast(`已选目录完成：导入或更新 ${totals.imported} 个视频${scrapeSummary(scraped)}`);
  }catch(error){
    finishScanProgress(backendErrorMessage(error), true);
    toast('扫描已选云盘目录失败：' + backendErrorMessage(error));
  }finally{
    guangyaSelectedFolders.clear();
    renderGuangyaBrowser();
  }
}
async function scanCurrentGuangyaFolder(){
  const source = activeBrowserSource();
  const folderName = guangyaBrowser.folderName || '根目录';
  if(isOpenListSource(source)) return addOpenListFolder(guangyaBrowser.parentId || '/', folderName);
  if(!resetScanProgress('cloud', `扫描云盘目录：${folderName}`)) return;
  logScanProgress(`开始递归扫描 ${source.name} · ${guangyaBrowserPath().join(' / ')}`);
  try{
    const rootId = guangyaBrowser.parentId === null || guangyaBrowser.parentId === undefined || String(guangyaBrowser.parentId) === ''
      ? null
      : String(guangyaBrowser.parentId);
    updateScanProgress('正在读取目录并导入视频', null);
    const report = await TtvBackend.invoke('provider_sync_library_recursive', {providerId:source.id, input:{rootId, pageSize:100, maxItems:100000}});
    scanProgress.folders = Number(report?.folders || 0);
    scanProgress.files = Number(report?.fetched || 0);
    scanProgress.imported = Number(report?.imported || 0);
    scanProgress.skipped = Number(report?.skipped || 0);
    scanProgress.promotional = Number(report?.skippedPromotional || 0);
    scanProgress.nonVideo = Number(report?.skippedNonVideo || 0);
    logScanProgress(`发现 ${scanProgress.files} 项，导入 ${scanProgress.imported} 个视频，过滤 ${scanProgress.skipped} 项（广告/推广 ${scanProgress.promotional}，非视频 ${scanProgress.nonVideo}）`);
    if(scanTaskPausedByUser()) return;
    updateScanProgress('视频已入库，开始刮削元数据', null);
    const scraped = await scrapeLibraryUntilDone(5000);
    if(scanTaskPausedByUser()) return;
    scanProgress.updated = Number(scraped?.updated || 0);
    updateScanProgress('刮削完成，刷新影视库', null);
    await refreshLibraryAfterImport();
    finishScanProgress(`${folderName} 已完成：影视库新增或更新 ${scanProgress.imported} 个视频，刮削 ${scanProgress.updated} 条。`);
    const warned = await maybeWarnTmdbMissing(scraped);
    if(!warned) toast(`${folderName}：已添加 ${scanProgress.imported} 个视频${scrapeSummary(scraped)}`);
  }catch(error){
    finishScanProgress(backendErrorMessage(error), true);
    toast('扫描云盘目录失败：' + backendErrorMessage(error));
  }
}
function previewGuangyaRename(){
  const format = document.getElementById('cloudRenameFormat')?.value.trim() || '{name}';
  const files = guangyaBrowser.entries.filter(item => item.kind !== 'folder');
  const preview = document.getElementById('cloudRenamePreview');
  if(!preview) return;
  if(!files.length){ preview.textContent = '当前目录没有可预览的文件。'; return; }
  preview.innerHTML = files.slice(0, 8).map(item => `${escapeHtml(item.name)} → ${escapeHtml(format.replace('{name}', item.name).replace('{tmdb-id}', '待匹配').replace('{year}', '—'))}`).join('<br>') + (files.length > 8 ? `<br>还有 ${files.length - 8} 项...` : '');
}
async function logoutGuangyaAccount(){
  if(!TtvBackend.available()) return;
  const source = activeCloudSource || activeBrowserSource();
  try{
    await TtvBackend.invoke('provider_logout', {providerId:source.id});
    resetCloudBrowserState(source.id);
    toast(`已退出${source.name}。`);
    if(source.id === 'guangya') loadGuangyaOAuthStatus();
    else loadProviderOAuthStatus(source);
  }catch(error){ toast('退出失败：' + backendErrorMessage(error)); }
}

async function loadGuangyaResources(parentId = null, folderName = '根目录', append = false){
  const source = activeBrowserSource();
  if(!TtvBackend.available()) return toast(`${source.name}文件浏览需要在桌面端运行。`);
  if(isOpenListSource(source)) return loadOpenListResources(parentId || '/', folderName, append);
  try{
    if(!append){
      guangyaBrowser.parentId = parentId;
      guangyaBrowser.folderName = folderName;
      guangyaBrowser.entries = [];
      guangyaBrowser.nextPageToken = null;
    }
    const input = {
      pageSize: 100,
      parentId: parentId === null || parentId === undefined ? '' : String(parentId),
      ...(append && guangyaBrowser.nextPageToken ? {pageToken:guangyaBrowser.nextPageToken} : {})
    };
    const page = await TtvBackend.invoke('provider_list_files', {providerId:source.id, input});
    guangyaBrowser.entries.push(...(Array.isArray(page?.files) ? page.files : []));
    guangyaBrowser.nextPageToken = page?.nextPageToken || null;
    renderGuangyaBrowser();
  }catch(error){
    const body = document.getElementById('cloudSourceBody');
    const message = backendErrorMessage(error);
    const expired = error?.code === 'provider_session_expired' || /session has expired|会话.*过期|令牌.*过期/i.test(message);
    if(body) body.innerHTML = expired
      ? `<div style="display:flex;flex-direction:column;gap:10px;text-align:left;font-size:12px;line-height:1.7"><div style="color:var(--danger)">${escapeHtml(source.name)}访问令牌已过期，自动刷新未成功。</div><div style="color:var(--text-faint)">请重新扫码授权；授权成功后会继续保留真实目录和媒体库操作。</div><button class="btn btn-accent" id="guangyaReloginFromError">重新二维码授权</button></div>`
      : `<p style="color:var(--danger);font-size:12px;line-height:1.7">无法读取${escapeHtml(source.name)}目录：${escapeHtml(message)}</p>`;
    document.getElementById('guangyaReloginFromError')?.addEventListener('click', () => startCloudScanLogin(source));
    toast(expired ? `${source.name}授权已过期，请重新扫码。` : `无法读取${source.name}目录：` + message);
  }
}
async function loadOpenListResources(path = '/', folderName = '根目录', append = false){
  const source = activeBrowserSource();
  const storage = openlistStorageForSource(source);
  if(!storage) return toast(`请先配置${source.name}的 OpenList 存储。`);
  try{
    if(!append){
      guangyaBrowser.parentId = path || '/';
      guangyaBrowser.folderName = folderName;
      guangyaBrowser.entries = [];
      guangyaBrowser.nextPageToken = null;
    }
    const page = await TtvBackend.invoke('openlist_list_files', {input:{
      storageId: String(storage.id),
      path: path || '/',
      pageSize: 100,
      ...(append && guangyaBrowser.nextPageToken ? {cursor:guangyaBrowser.nextPageToken} : {})
    }});
    const entries = Array.isArray(page?.files) ? page.files.map(item => ({
      id: item.id || item.path,
      name: item.name,
      kind: item.isFolder ? 'folder' : 'file',
      parentId: path || '/',
      sizeBytes: item.size,
      mimeType: item.mimeType,
      thumbnailUrl: item.thumbnailUrl,
      path: item.path
    })) : [];
    guangyaBrowser.entries.push(...entries);
    guangyaBrowser.nextPageToken = page?.nextCursor || null;
    renderGuangyaBrowser();
  }catch(error){
    const body = document.getElementById('cloudSourceBody');
    const message = backendErrorMessage(error);
    if(body) body.innerHTML = `<p style="color:var(--danger);font-size:12px;line-height:1.7">无法读取${escapeHtml(source.name)}目录：${escapeHtml(message)}</p>`;
    toast(`无法读取${source.name}目录：${message}`);
  }
}
async function addGuangyaFolder(folderId, folderName){
  const source = activeBrowserSource();
  if(isOpenListSource(source)) return addOpenListFolder(folderId, folderName);
  if(folderId === null || folderId === undefined || String(folderId) === '') return toast('请选择具体文件夹后再添加。');
  if(guangyaImportingFolders.has(folderId)) return toast(`${folderName} 正在扫描中。`);
  const markAdult = await chooseAdultMarkForImport(folderName);
  if(markAdult === undefined) return;
  guangyaImportingFolders.add(folderId);
  if(!scanProgress.active){
    if(!resetScanProgress('cloud', `扫描云盘目录：${folderName}`)){
      guangyaImportingFolders.delete(folderId);
      return;
    }
  }else if(scanProgress.kind !== 'cloud'){
    guangyaImportingFolders.delete(folderId);
    toast('已有扫描或刮削任务正在运行，请等待完成或先在通知中心暂停。');
    return;
  }else{
    document.getElementById('scanProgressTitle') && (document.getElementById('scanProgressTitle').textContent = `扫描云盘目录：${folderName}`);
  }
  logScanProgress(`开始读取云盘目录：${folderName}`);
  try{
    let fetched = 0, imported = 0, skipped = 0, folderCount = 0;
    const visited = new Set();
    const walk = async (currentId, folderPath) => {
      if(currentId === null || currentId === undefined || String(currentId) === '' || visited.has(String(currentId)) || visited.size > 1000) return;
      currentId = String(currentId);
      visited.add(currentId);
      folderCount++;
      let pageToken = null;
      const children = [];
      const pageSeen = new Set();
      for(let page = 0; page < 100; page++){
        const result = await TtvBackend.invoke('provider_list_files', {providerId:source.id, input:{parentId:currentId, pageSize:100, ...(pageToken ? {pageToken} : {})}});
        const entries = Array.isArray(result?.files) ? result.files : [];
        children.push(...entries);
        const next = result?.nextPageToken || null;
        if(!next || pageSeen.has(next) || next === pageToken) break;
        pageSeen.add(next); pageToken = next;
      }
      fetched += children.length;
      scanProgress.folders = folderCount;
      scanProgress.files = fetched;
      scanProgress.skipped = skipped;
      updateScanProgress(`正在扫描 ${folderName} · 已读取 ${fetched} 项`, null);
      let syncToken = null;
      const syncSeen = new Set();
      for(let page = 0; page < 100; page++){
        const report = await TtvBackend.invoke('provider_sync_library', {providerId:source.id, input:{parentId:currentId, pageSize:100, ...(syncToken ? {pageToken:syncToken} : {}), ...(folderPath ? {folderPath} : {}), ...(markAdult === null ? {} : {markAdult})}});
        imported += Number(report?.imported || 0);
        skipped += Number(report?.skipped || 0);
        scanProgress.promotional += Number(report?.skippedPromotional || 0);
        scanProgress.nonVideo += Number(report?.skippedNonVideo || 0);
        scanProgress.imported = imported;
        scanProgress.skipped = skipped;
        updateScanProgress(`正在导入视频 · 已入库 ${imported} 个`, null);
        const next = report?.nextPageToken || null;
        if(!next || syncSeen.has(next) || next === syncToken) break;
        syncSeen.add(next); syncToken = next;
      }
      for(const child of children.filter(item => item.kind === 'folder')) await walk(child.id, folderPath ? `${folderPath}/${child.name}` : child.name);
    };
    await walk(folderId, folderName);
    if(scanTaskPausedByUser()) return;
    updateScanProgress('云盘导入完成，开始刮削元数据', null);
    const scraped = await scrapeLibraryUntilDone(5000);
    if(scanTaskPausedByUser()) return;
    scanProgress.updated = Number(scraped?.updated || 0);
    updateScanProgress('刮削完成，刷新影视库', null);
    await refreshLibraryAfterImport();
    finishScanProgress(`${folderName} 已完成：导入 ${imported} 个视频，刮削 ${scanProgress.updated} 条。`);
    toast(`${folderName}：扫描 ${folderCount} 个文件夹，发现 ${fetched} 项，已添加 ${imported} 个视频${skipped ? `，过滤 ${skipped} 项非视频/文件夹` : ''}${scrapeSummary(scraped)}`);
  }catch(error){
    finishScanProgress(backendErrorMessage(error), true);
    toast('添加文件夹失败：' + backendErrorMessage(error));
  }finally{
    guangyaImportingFolders.delete(folderId);
    renderGuangyaBrowser();
  }
}
async function addOpenListFolder(folderPath, folderName){
  const source = activeBrowserSource();
  const storage = openlistStorageForSource(source);
  if(!storage || !folderPath) return toast('请选择具体文件夹后再添加。');
  if(!resetScanProgress('openlist', `扫描 OpenList：${folderName}`)) return;
  logScanProgress(`开始读取 OpenList 目录：${folderName}`);
  try{
    updateScanProgress('正在扫描 OpenList 目录', null);
    const report = await TtvBackend.invoke('openlist_sync_library', {input:{storageId:String(storage.id), path:String(folderPath), maxItems:100000}});
    scanProgress.folders = Number(report?.folders || 0);
    scanProgress.files = Number(report?.fetched || 0);
    scanProgress.imported = Number(report?.imported || 0);
    scanProgress.skipped = Number(report?.skipped || 0);
    scanProgress.promotional = Number(report?.skippedPromotional || 0);
    scanProgress.nonVideo = Number(report?.skippedNonVideo || 0);
    updateScanProgress('OpenList 导入完成，开始刮削元数据', null);
    const scraped = await scrapeLibraryUntilDone(5000);
    if(scanTaskPausedByUser()) return;
    scanProgress.updated = Number(scraped?.updated || 0);
    await refreshLibraryAfterImport();
    finishScanProgress(`${folderName} 已完成：导入 ${scanProgress.imported} 个视频，刮削 ${scanProgress.updated} 条。`);
    toast(`${folderName}：发现 ${Number(report?.fetched || 0)} 项，已添加 ${Number(report?.imported || 0)} 个视频${report?.skipped ? `，过滤 ${report.skipped} 项` : ''}${scrapeSummary(scraped)}`);
  }catch(error){
    finishScanProgress(backendErrorMessage(error), true);
    toast('添加 OpenList 文件夹失败：' + backendErrorMessage(error));
  }
}
async function renderOfficialQr(targetId, value){
  const target = document.getElementById(targetId);
  if(!target) return;
  target.textContent = '正在生成官方二维码...';
  try{
    if(!window.TtvQrCode) await new Promise(resolve => window.setTimeout(resolve, 80));
    if(!window.TtvQrCode) throw new Error('二维码组件尚未加载');
    const canvas = document.createElement('canvas');
    await window.TtvQrCode.toCanvas(canvas, value, {
      width: 172,
      margin: 2,
      errorCorrectionLevel: 'M',
      color: {
        dark: '#0a0e17',
        light: '#e2e8f0'
      }
    });
    canvas.style.borderRadius = '14px';
    canvas.style.display = 'block';
    canvas.style.boxShadow = '0 8px 30px rgba(0,0,0,0.5), inset 0 0 0 1px rgba(255,255,255,0.1)';
    target.replaceChildren(canvas);
  }catch(error){
    target.innerHTML = `<span style="color:var(--danger)">二维码生成失败：${escapeHtml(backendErrorMessage(error))}</span>`;
  }
}
async function startCloudScanLogin(source){
  if(source.id === 'guangya') return startGuangyaScanLogin();
  const capabilities = await loadProviderCapabilities(source.id);
  if(capabilities?.deviceCodeLogin === true){
    return startProviderDeviceQrLogin(source);
  }
  // These vendors generate and poll QR state only inside their own first-party
  // page. Keep that session in an isolated Tauri WebView instead of trying to
  // recreate private cookie/CAS protocols in our backend.
  return openOfficialProviderPage(source, 'login');
}

async function openOfficialProviderPage(source, page = 'login'){
  const body = document.getElementById('cloudSourceBody');
  try{
    const label = await TtvBackend.invoke('provider_open_official_page', {
      providerId: source.id,
      page
    });
    if(body) body.innerHTML = `<div class="official-page-handoff">
      <div class="official-page-icon">${sourceVisual(source.name).html}</div>
      <strong>${escapeHtml(source.name)} 官方登录页已打开</strong>
      <span>请在弹出的官方窗口中完成二维码扫码，登录状态由服务商页面维护。</span>
      <button class="btn btn-ghost" id="providerOfficialReopen">重新打开登录窗口</button>
    </div>`;
    document.getElementById('providerOfficialReopen')?.addEventListener('click', () => openOfficialProviderPage(source, page));
    toast(`${source.name} 官方二维码登录窗口已打开。`);
    return label;
  }catch(error){
    if(body) body.innerHTML = `<div style="color:var(--danger);font-size:12px;line-height:1.7">无法打开${escapeHtml(source.name)}官方登录页：${escapeHtml(backendErrorMessage(error))}</div>`;
    toast('打开官方登录页失败：' + backendErrorMessage(error));
  }
}
async function startProviderDeviceQrLogin(source){
  const body = document.getElementById('cloudSourceBody');
  const action = document.getElementById('cloudSourceAction');
  clearProviderQrPoll();
  try{
    const device = await TtvBackend.invoke('provider_qr_login_create', {providerId: source.id});
    const url = device?.qrText;
    if(!url) throw new Error('后端未返回官方二维码内容');
    if(action){ action.textContent = '正在等待授权'; action.onclick = () => {}; }
    const qrId = 'providerOfficialQr';
    const statusId = 'providerQrPollStatus';
    if(body) body.innerHTML = `
      <div class="official-qr-wrap">
        <div class="qr-sub-tip">请使用 <b>${escapeHtml(source.name)}</b> 官方客户端扫码授权</div>
        <div class="qr-canvas-holder">
          <div id="${qrId}" class="qr-canvas-box"></div>
        </div>
        <div class="qr-device-code-chip">
          <span>设备码</span>
          <strong>${escapeHtml(device.userCode || '未提供')}</strong>
        </div>
        <div id="${statusId}" class="qr-poll-badge"><span class="dot-pulse"></span> 等待授权确认...</div>
        <button class="btn btn-ghost qr-cancel-btn" id="providerQrCancel">取消</button>
      </div>`;
    renderOfficialQr(qrId, url);
    document.getElementById('providerQrCancel')?.addEventListener('click', () => { clearProviderQrPoll(); selectCloudSource(source.name); });
    pollProviderDeviceQr(source, device.sessionId, Math.max(3, Number(device.interval) || 5));
  }catch(error){
    clearProviderQrPoll();
    if(body) body.innerHTML = `<div style="color:var(--danger);font-size:12px;line-height:1.7;text-align:left">${escapeHtml(source.name)} 的二维码登录无法启动：${escapeHtml(backendErrorMessage(error))}<br><span style="color:var(--text-faint)">请使用官方 OAuth 浏览器授权，不会生成模拟二维码。</span></div>`;
    if(action){ action.textContent = '二维码 OAuth 授权'; action.onclick = () => startOAuthFlow(source); }
  }
}
async function pollProviderDeviceQr(source, sessionId, interval){
  try{
    const result = await TtvBackend.invoke('provider_qr_login_poll', {providerId: source.id, input:{sessionId}});
    const pollStatus = document.getElementById('providerQrPollStatus');
    if(result.status === 'authorized'){
      clearProviderQrPoll();
      toast(source.name + ' 已连接，令牌已安全保存。');
      return selectCloudSource(source.name);
    }
    if(result.status === 'denied' || result.status === 'expired'){
      clearProviderQrPoll();
      if(pollStatus) pollStatus.textContent = result.status === 'denied' ? '授权被拒绝。' : '设备码已过期。';
      return;
    }
    if(pollStatus) pollStatus.textContent = result.status === 'slowDown' ? '服务端要求放慢轮询。' : '等待授权确认...';
    const nextInterval = Math.max(3, Number(result.interval) || interval);
    providerQrPollTimer = window.setTimeout(() => pollProviderDeviceQr(source, sessionId, nextInterval), nextInterval * 1000);
  }catch(error){
    clearProviderQrPoll();
    const pollStatus = document.getElementById('providerQrPollStatus');
    if(pollStatus) pollStatus.textContent = '授权状态检查失败：' + backendErrorMessage(error);
  }
}
async function startGuangyaScanLogin(){
  const status = await TtvBackend.invoke('guangya_oauth_status');
  if(!status.configured) return loadGuangyaOAuthStatus();
  return startGuangyaDeviceOAuth();
}
async function startGuangyaDeviceOAuth(){
  const body = document.getElementById('cloudSourceBody');
  const action = document.getElementById('cloudSourceAction');
  clearGuangyaOAuthPoll();
  try{
    const device = await TtvBackend.invoke('provider_qr_login_create', {providerId:'guangya'});
    const url = device.qrText;
    if(action){ action.textContent = '正在等待授权'; action.onclick = () => {}; }
    if(body) body.innerHTML = `
      <div class="official-qr-wrap">
        <div class="qr-sub-tip">请使用 <b>光鸭官方客户端</b> 扫描二维码完成授权</div>
        <div class="qr-canvas-holder">
          <div id="guangyaOfficialQr" class="qr-canvas-box"></div>
        </div>
        <div class="qr-device-code-chip">
          <span>设备码</span>
          <strong>${escapeHtml(device.userCode)}</strong>
        </div>
        <div id="guangyaOAuthPollStatus" class="qr-poll-badge"><span class="dot-pulse"></span> 等待授权确认...</div>
        <button class="btn btn-ghost qr-cancel-btn" id="guangyaOAuthCancel">取消</button>
      </div>`;
    renderOfficialQr('guangyaOfficialQr', url);
    document.getElementById('guangyaOAuthCancel')?.addEventListener('click', loadGuangyaOAuthStatus);
    pollGuangyaDeviceOAuth(device.sessionId, Math.max(3, Number(device.interval) || 5));
  }catch(error){
    toast('光鸭设备授权无法启动：' + backendErrorMessage(error));
    loadGuangyaOAuthStatus();
  }
}
async function pollGuangyaDeviceOAuth(deviceCode, interval){
  try{
    const result = await TtvBackend.invoke('provider_qr_login_poll', {providerId:'guangya', input:{sessionId:deviceCode}});
    const pollStatus = document.getElementById('guangyaOAuthPollStatus');
    if(result.status === 'authorized'){
      clearGuangyaOAuthPoll();
      toast('光鸭 OAuth 已连接，令牌已安全保存。');
      return loadGuangyaOAuthStatus();
    }
    if(result.status === 'denied' || result.status === 'expired'){
      clearGuangyaOAuthPoll();
      if(pollStatus) pollStatus.textContent = result.status === 'denied' ? '授权被拒绝。' : '设备码已过期。';
      return;
    }
    if(pollStatus) pollStatus.textContent = result.status === 'slowDown' ? '服务端要求放慢轮询。' : '等待授权确认...';
    guangyaOAuthPollTimer = window.setTimeout(() => pollGuangyaDeviceOAuth(deviceCode, result.interval || interval), (result.interval || interval) * 1000);
  }catch(error){
    clearGuangyaOAuthPoll();
    const pollStatus = document.getElementById('guangyaOAuthPollStatus');
    if(pollStatus) pollStatus.textContent = '授权状态检查失败：' + backendErrorMessage(error);
  }
}
async function startOAuthFlow(source, options = {}){
  const body = document.getElementById('cloudSourceBody');
  const action = document.getElementById('cloudSourceAction');
  if(!TtvBackend.available()){
    if(body) body.innerHTML = '<p style="color:var(--text-faint);font-size:12px;line-height:1.7">OAuth 登录需要在 TTV Box 桌面端执行。</p>';
    return;
  }
  try{
    const state = (crypto.randomUUID ? crypto.randomUUID() : String(Date.now()));
    const url = await TtvBackend.invoke('provider_oauth_authorization_url', {providerId: source.id, input: {state}});
    if(action){ action.textContent = '正在等待授权'; action.onclick = () => {}; }
    const qrId = `oauthQr-${source.id}`;
    if(body) body.innerHTML = `
      <div class="official-qr-wrap">
        <div class="qr-sub-tip">请扫描二维码进入 <b>${escapeHtml(source.name)}</b> 官方 OAuth 授权页</div>
        <div class="qr-canvas-holder"><div id="${qrId}" class="qr-canvas-box"></div></div>
        <div class="qr-device-code-chip"><span>授权模式</span><strong>官方 OAuth 2.0</strong></div>
        <div class="qr-poll-badge">完成授权后，将服务商返回的 authorization code 填写在下方</div>
        <input class="modal-input" id="oauthCodeInput" placeholder="authorization code" autocomplete="off">
        <button class="btn btn-accent" id="oauthExchangeButton">确认授权并连接</button>
        <button class="btn btn-ghost qr-cancel-btn" id="oauthQrCancel">取消</button>
      </div>`;
    renderOfficialQr(qrId, url);
    document.getElementById('oauthQrCancel')?.addEventListener('click', () => selectCloudSource(source.name));
    document.getElementById('oauthExchangeButton')?.addEventListener('click', async () => {
      const code = document.getElementById('oauthCodeInput')?.value.trim();
      if(!code) return toast('请先填写官方授权码。');
      try{
        await TtvBackend.invoke('provider_oauth_exchange_code', {providerId: source.id, input: {code}});
        toast(source.name + ' OAuth 令牌已安全保存。');
        selectCloudSource(source.name);
      }catch(error){ toast('OAuth 兑换失败：' + backendErrorMessage(error)); }
    });
    toast(`${source.name} 官方授权二维码已显示在当前窗口。`);
  }catch(error){
    if(action){ action.textContent = '二维码授权登录'; action.onclick = () => startCloudScanLogin(source); }
    const envKey = `TTV_OAUTH_${String(source.id || '').toUpperCase()}_CLIENT_ID`;
    const configExample = JSON.stringify({oauth: {[source.id]: {clientId: '你的官方 OAuth Client ID'}}}, null, 2);
    const privateSessionOnly = ['quark', '115'].includes(source.id);
    if(body) body.innerHTML = `
      <div style="display:flex;flex-direction:column;gap:10px;text-align:left;font-size:12px;line-height:1.7">
        <div style="color:var(--danger)">无法启动真实二维码授权：${escapeHtml(backendErrorMessage(error))}</div>
        ${privateSessionOnly
          ? `<div style="color:var(--text-faint)">${escapeHtml(source.name)} 当前没有已验证的第三方设备码/OAuth 授权协议。应用不会复制官网 Cookie、私有 CAS、客户端密钥或设备签名，也不会显示无法授权给本应用的模拟二维码。</div>`
          : `<div style="color:var(--text-faint)">要在当前窗口生成真实可用的官方授权二维码，需要先使用你为本应用在 ${escapeHtml(source.name)} 开放平台登记的 Client ID。</div><pre style="margin:0;overflow:auto;padding:10px;background:var(--bg-deep);border:1px solid var(--border);border-radius:5px;color:var(--text);font-size:11px">${escapeHtml(configExample)}</pre><div style="color:var(--text-faint)">也可以在启动程序前设置环境变量 <code>${escapeHtml(envKey)}</code>，然后完全退出并重新启动桌面程序。</div>`}
      </div>`;
  }
}

function backendErrorMessage(error){
  const text = error && typeof error === 'object' && error.message
    ? String(error.message)
    : String(error || '请求失败');
  if(/111104|设备身份无效/.test(text)){
    return '红果设备身份无效。请从真机抓包更新 deviceId / installId，并写入服务端下发的 deviceToken（x-tt-dt）。';
  }
  if(/110001/.test(text)){
    return '红果播放模型异常。漫剧请使用 App V2 接口，或更换设备凭据后重试。';
  }
  return text;
}
function formatDuration(seconds){
  if(!Number.isFinite(seconds) || seconds <= 0) return '时长未知';
  const minutes = Math.round(seconds / 60);
  if(minutes < 60) return minutes + ' 分钟';
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? hours + ' 小时 ' + rest + ' 分' : hours + ' 小时';
}
/** JavBus 等源在缺时长时会填 60 分钟，不能当真片长。 */
function isPlaceholderDurationMin(minutes){
  return Number(minutes) === 60;
}
function resolveMediaDurationSeconds(media, javCard){
  const fileSeconds = Number(media?.durationSeconds);
  if(Number.isFinite(fileSeconds) && fileSeconds > 0 && fileSeconds !== 3600) return fileSeconds;
  const minutes = Number(javCard?.durationMin);
  if(minutes > 0 && !isPlaceholderDurationMin(minutes)) return minutes * 60;
  return 0;
}
function detectMediaQualityLabel(source){
  const text = String(source || '');
  const match = text.match(/\b(2160p|1080p|1080i|720p|480p|4k|8k|uhd)\b/i)
    || text.match(/[\[【(](4k|8k|uhd|2160p|1080p|720p|480p|hd|sd)[\]】)]/i);
  if(!match) return '';
  const token = String(match[1]).toUpperCase();
  if(token === '2160P' || token === 'UHD' || token === '4K') return '4K';
  if(token === '1080P' || token === '1080I') return '1080P';
  if(token === '720P') return '720P';
  if(token === '480P' || token === 'SD') return '480P';
  if(token === '8K') return '8K';
  if(token === 'HD') return 'HD';
  return token;
}
function positiveNumber(value){
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? number : 0;
}
function isStreamHubShow(movie){
  return movie?.providerId === 'streamhub' && /^(show|tv|series|tv_show):/i.test(String(movie.providerMediaId || ''));
}
function episodeRecord(raw){
  return raw?.metadata?.streamhub && typeof raw.metadata.streamhub === 'object'
    ? raw.metadata.streamhub
    : (raw?.streamhub && typeof raw.streamhub === 'object' ? raw.streamhub : raw || {});
}
function episodeProviderMediaId(raw, record){
  const direct = raw?.providerMediaId || raw?.mediaId || (String(raw?.id || '').startsWith('file:') ? raw.id : '');
  const versions = Array.isArray(record?.versions) ? record.versions : [];
  const selectedVersion = versions.find(version => version?.selected === true) || versions[0] || {};
  const candidate = direct || record?.providerMediaId || record?.mediaFileId || selectedVersion.mediaFileId;
  if(!candidate) return '';
  const value = String(candidate).trim();
  if(!value) return '';
  return /^(file|movie|show|tv|series):/i.test(value) ? value : 'file:' + value;
}
function normalizeEpisodes(rawEpisodes, parentMovie){
  if(!Array.isArray(rawEpisodes)) return [];
  return rawEpisodes.map((raw, index) => {
    const record = episodeRecord(raw);
    const providerMediaId = episodeProviderMediaId(raw, record);
    const seasonNumber = positiveNumber(record.seasonNumber ?? raw?.seasonNumber);
    const episodeNumber = positiveNumber(record.episodeNumber ?? raw?.episodeNumber);
    const title = String(record.title || raw?.name || '').trim();
    const stablePart = record.id || raw?.id || providerMediaId || `${seasonNumber}-${episodeNumber}-${index}`;
    const durationSeconds = positiveNumber(raw?.durationSeconds ?? record.durationSeconds);
    return {
      id: String(parentMovie?.id || 'media') + ':episode:' + stablePart,
      providerId: parentMovie?.providerId || 'streamhub',
      providerMediaId,
      playUrl: raw?.playUrl || record.playUrl || record.url || '',
      playHeaders: raw?.playHeaders || record.playHeaders || {},
      title: title || (episodeNumber ? `第 ${String(episodeNumber).padStart(2, '0')} 集` : '未命名剧集'),
      seasonNumber,
      episodeNumber,
      durationSeconds,
      durationLabel: formatDuration(durationSeconds),
      img: raw?.thumbnailUrl || raw?.artUrl || record.stillPath || parentMovie?.img || '/assets/detail-poster.jpg',
      summary: record.overview || raw?.summary || parentMovie?.summary || '',
      raw
    };
  }).filter(episode => episode.providerMediaId || episode.playUrl)
    .sort((left, right) => left.seasonNumber - right.seasonNumber || left.episodeNumber - right.episodeNumber || left.id.localeCompare(right.id));
}
function isEphemeralArtworkUrl(value){
  const text = String(value || '').trim();
  if(!/^https?:/i.test(text)) return false;
  try{
    const url = new URL(text);
    return ['auth_key', 'authkey', 'token', 'signature', 'sig', 'expires', 'x-amz-signature']
      .some(key => url.searchParams.has(key));
  }catch(error){
    return /[?&](?:auth_key|authkey|token|signature|sig|expires|x-amz-signature)=/i.test(text);
  }
}
function normalizeArtworkUrl(value, fallback = '/assets/detail-poster.jpg'){
  const text = String(value || '').trim();
  if(!text) return fallback;
  // Provider thumbnails with temporary signatures are playback-session data,
  // not durable library artwork. Persisting them creates 404 storms on restart.
  if(isEphemeralArtworkUrl(text)) return fallback;
  if(/^(https?:|blob:|data:|asset:)/i.test(text)) return text;
  if(/^assets\//i.test(text)) return text;
  if(window.TtvConvertFileSrc && (/^file:\/\//i.test(text) || /^[A-Za-z]:[\\/]/.test(text) || /^\\\\/.test(text))){
    try{
      const localPath = /^file:\/\//i.test(text)
        ? decodeURIComponent(new URL(text).pathname.replace(/^\/+/, '').replace(/^([A-Za-z]):\//, '$1:/'))
        : text;
      return window.TtvConvertFileSrc(localPath);
    }catch(error){ console.warn('Unable to convert artwork path:', error); }
  }
  // Provider-relative paths (for example /cache/images/...) are only valid
  // while that provider is reachable; use a local poster until then.
  if(text.startsWith('/')) return fallback;
  return text;
}
/* 无封面条目的占位：渐变底 + 居中标题（替代把 alt 文字挤成竖排的破图）。 */
const FILM_ICON_SVG = '<svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M7 4v16M17 4v16M3 9h4M3 15h4M17 9h4M17 15h4"/></svg>';
function posterMarkup(m, alt = ''){
  const label = alt || escapeHtml(m?.t || '未命名媒体');
  if(m && m.hasArtwork && m.img){
    return `<img class="card-cover is-pending" data-cover-src="${m.img}" alt="${label}" width="400" height="600" loading="lazy" decoding="async"/>`;
  }
  const metaParts = [
    m && m.y && m.y !== '—' ? m.y : '',
    m && m.genre && m.genre !== '未分类' ? m.genre : ''
  ].filter(Boolean);
  return `<div class="art-placeholder" role="img" aria-label="${label}"><span class="art-placeholder-icon">${FILM_ICON_SVG}</span><span class="art-placeholder-title">${escapeHtml(m?.t || '未命名媒体')}</span>${metaParts.length ? `<span class="art-placeholder-meta">${escapeHtml(metaParts.join(' · '))}</span>` : ''}</div>`;
}
function cleanMediaDisplayTitle(value){
  const raw = String(value || '').trim();
  if(!raw) return '未命名媒体';
  const stem = raw.split(/[\\/]/).pop().replace(/\.(mkv|mp4|avi|mov|m4v|webm|wmv|ts|m2ts|flv|rmvb?)$/i, '');
  const readable = stem.replace(/[._]+/g, ' ').replace(/\s+/g, ' ').trim();
  const episode = readable.match(/\bS\d{1,3}E\d{1,4}\b/i);
  let title = episode ? readable.slice(0, episode.index) : readable;
  if(!episode){
    title = title.split(/\b(?:2160p|1080[pi]|720p|480p|WEB[ .-]?DL|WEBRip|BluRay|REMUX|REPACK|HDTV|HDR10\+?|HDR|DV|HEVC|AVC|H[ .-]?26[45]|X26[45]|DDP\d*|DTS(?:-HD)?|AAC|\d{2,3}FPS|DVDRip|LDVDRip|BDRip|TVRip|R\d|HALFCD|\dAudios?|\{tmdb[-_]?\d+\})\b/i)[0];
  }
  title = title
    .replace(/\b(?:19|20)\d{2}\b/g, ' ')
    // Dropping the year used to strand empty parentheses ("007：大破量子危机 ( ) -");
    // remove them together with any leftover connector.
    .replace(/\(\s*\)|（\s*）/g, ' ')
    .replace(/\s+/g, ' ')
    // Trailing separators (" -", " ·", "_") are release-name residue.
    .replace(/[\s·\-–—_:：]+$/u, '')
    .trim();
  if(!title) title = readable;
  return episode ? `${title} · ${episode[0].toUpperCase()}` : title;
}
function parseLibraryEpisodeIdentity(media, payload, sourceTitle){
  const identity = parseEpisodeIdentity(sourceTitle) || parseEpisodeIdentity(media?.title) || parseEpisodeIdentity(media?.remotePath);
  if(!identity) return null;
  const weak = !identity.seriesTitle || /^(S\d+E\d+|第\s*\d+\s*[集话])$/i.test(identity.seriesTitle);
  const folder = String(payload?.folderName || '').trim();
  if(weak && folder && folder !== '根目录') identity.seriesTitle = folder;
  return identity;
}
function parseEpisodeIdentity(value){
  const raw = String(value || '').trim();
  if(!raw) return null;
  const stem = raw.split(/[\\/]/).pop().replace(/\.(mkv|mp4|avi|mov|m4v|webm|wmv|ts|m2ts|flv|rmvb?|mpeg|mpg)$/i, '');
  const readable = stem.replace(/[._]+/g, ' ').replace(/\s+/g, ' ').trim();
  const patterns = [
    {regex: /(?:^|[\s·_-])S(\d{1,3})E(\d{1,4})(?:E\d{1,4})?\b/i, season:1, episode:2},
    {regex: /(?:^|[\s·_-])(\d{1,3})x(\d{1,4})\b/i, season:1, episode:2},
    {regex: /第\s*(\d{1,3})\s*季\s*第\s*(\d{1,4})\s*[集话]/i, season:1, episode:2},
    {regex: /第\s*(\d{1,4})\s*[集话]/i, season:null, episode:1}
  ];
  for(const pattern of patterns){
    const match = readable.match(pattern.regex);
    if(!match) continue;
    const before = readable.slice(0, match.index).replace(/[\s·_-]+$/g, '').trim();
    const after = readable.slice((match.index || 0) + match[0].length)
      .split(/\b(?:2160p|1080[pi]|720p|480p|WEB[ .-]?DL|WEBRip|BluRay|REMUX|REPACK|HDTV|HDR10\+?|HDR|DV|HEVC|AVC|H[ .-]?26[45]|X26[45]|DDP\d*|DTS(?:-HD)?|AAC)\b/i)[0]
      .replace(/^[\s·_-]+|[\s·_-]+$/g, '')
      .trim();
    const seriesTitle = (before || cleanMediaDisplayTitle(readable))
      .replace(/\b(?:19|20)\d{2}\b\s*$/g, '')
      .trim();
    if(!seriesTitle) return null;
    return {
      seriesTitle,
      seasonNumber: pattern.season ? positiveNumber(match[pattern.season]) : 1,
      episodeNumber: positiveNumber(match[pattern.episode]),
      episodeTitle: after
    };
  }
  return null;
}
function attachArtworkFallback(image, fallback = '/assets/detail-poster.jpg', owner = null){
  if(!image) return;
  image.__artOwner = owner;
  image.__artFallback = fallback;
  if(image.dataset.artBound === '1') return;
  image.dataset.artBound = '1';
  image.addEventListener('error', () => {
    const currentOwner = image.__artOwner;
    const currentFallback = image.__artFallback || '/assets/detail-poster.jpg';
    const remote = currentOwner && typeof currentOwner === 'object' ? currentOwner.artRemote : '';
    if(remote && image.dataset.artRemoteTried !== '1' && remote !== image.getAttribute('src')){
      image.dataset.artRemoteTried = '1';
      if(currentOwner) currentOwner.img = remote;
      image.src = remote;
      return;
    }
    if(image.dataset.artFallback === '1') return;
    image.dataset.artFallback = '1';
    if(currentOwner) currentOwner.img = currentFallback;
    image.src = currentFallback;
  });
}
const coverLoadQueue = [];
let coverInFlight = 0;
let coverObserver = null;
function cardNearViewport(el, slackFactor = 1.1){
  if(!el || typeof el.getBoundingClientRect !== 'function') return false;
  const rect = el.getBoundingClientRect();
  const view = window.innerHeight || 800;
  const slack = view * slackFactor;
  return rect.bottom > -48 && rect.top < view + slack;
}
function ensureCoverObserver(){
  if(coverObserver) return coverObserver;
  if(typeof IntersectionObserver !== 'function') return null;
  coverObserver = new IntersectionObserver(entries => {
    for(const entry of entries){
      if(!entry.isIntersecting) continue;
      coverObserver.unobserve(entry.target);
      enqueueCoverLoad(entry.target, false);
    }
  }, {root:null, rootMargin:COVER_LAZY_ROOT_MARGIN, threshold:0});
  return coverObserver;
}
function prepareCoverImage(image, owner, fallback){
  if(!image) return '';
  attachArtworkFallback(image, fallback || '/assets/detail-poster.jpg', owner);
  image.decoding = 'async';
  image.classList.add('card-cover');
  if(!image.getAttribute('width')) image.setAttribute('width', '400');
  if(!image.getAttribute('height')) image.setAttribute('height', '600');
  const src = String(image.dataset.coverSrc || image.getAttribute('src') || '').trim();
  if(src && src !== COVER_PLACEHOLDER) image.dataset.coverSrc = src;
  return image.dataset.coverSrc || '';
}
function enqueueCoverLoad(image, eager){
  if(!image || image.dataset.coverLoaded === '1' || image.dataset.coverQueued === '1') return;
  image.dataset.coverQueued = '1';
  if(eager) coverLoadQueue.unshift(image);
  else coverLoadQueue.push(image);
  pumpCoverQueue();
}
function pumpCoverQueue(){
  while(coverInFlight < COVER_MAX_CONCURRENT && coverLoadQueue.length){
    const image = coverLoadQueue.shift();
    if(!image) continue;
    if(!image.isConnected){
      // First-page eager covers are bound while the card is still in a detached
      // fragment (before the grid is populated). Don't drop them: defer once to the
      // next frame so the append has happened, then re-pump. A card that never gets
      // connected (e.g. a superseded render) is dropped after one retry so it can't
      // starve the queue.
      if(image.dataset.coverDeferred === '1') continue;
      image.dataset.coverDeferred = '1';
      coverLoadQueue.unshift(image);
      if(typeof requestAnimationFrame === 'function') requestAnimationFrame(pumpCoverQueue);
      else setTimeout(pumpCoverQueue, 0);
      return;
    }
    const src = String(image.dataset.coverSrc || '').trim();
    if(!src || src === COVER_PLACEHOLDER){
      image.dataset.coverLoaded = '1';
      image.classList.remove('is-pending');
      continue;
    }
    if(image.getAttribute('src') === src){
      image.dataset.coverLoaded = '1';
      image.classList.remove('is-pending');
      continue;
    }
    coverInFlight++;
    const finish = () => {
      coverInFlight = Math.max(0, coverInFlight - 1);
      image.dataset.coverLoaded = '1';
      image.classList.remove('is-pending');
      pumpCoverQueue();
    };
    image.addEventListener('load', finish, {once:true});
    image.addEventListener('error', finish, {once:true});
    image.src = src;
  }
}
function bindCardCover(image, {eager = false, owner = null, fallback = '/assets/detail-poster.jpg'} = {}){
  if(!image) return;
  const src = prepareCoverImage(image, owner, fallback);
  if(!src){
    image.classList.remove('is-pending');
    return;
  }
  image.classList.add('is-pending');
  if(eager){
    image.loading = 'eager';
    image.setAttribute('fetchpriority', 'high');
    enqueueCoverLoad(image, true);
    return;
  }
  image.loading = 'lazy';
  image.removeAttribute('fetchpriority');
  if(image.getAttribute('src') !== COVER_PLACEHOLDER) image.src = COVER_PLACEHOLDER;
  const observer = ensureCoverObserver();
  if(observer) observer.observe(image);
  else enqueueCoverLoad(image, false);
}
function bindFlipCardCovers(card, owner, {eagerFront = false} = {}){
  if(!card) return;
  const front = card.querySelector('.fc-front img');
  const back = card.querySelector('.fc-back-bg img');
  if(front) bindCardCover(front, {eager:eagerFront, owner});
  if(!back) return;
  prepareCoverImage(back, owner);
  back.classList.add('is-pending');
  if(back.getAttribute('src') && back.getAttribute('src') !== COVER_PLACEHOLDER){
    back.dataset.coverSrc = back.dataset.coverSrc || back.getAttribute('src');
    back.src = COVER_PLACEHOLDER;
  }
  const loadBack = () => bindCardCover(back, {eager:true, owner});
  card.addEventListener('pointerenter', loadBack, {once:true});
  card.addEventListener('focusin', loadBack, {once:true});
}
const TMDB_GENRE_LABELS = {
  12:'冒险',14:'奇幻',16:'动画',18:'剧情',27:'恐怖',28:'动作',35:'喜剧',36:'历史',37:'西部',53:'惊悚',80:'犯罪',99:'纪录片',878:'科幻',9648:'悬疑',10402:'音乐',10749:'爱情',10751:'家庭',10752:'战争',10770:'电视电影',10759:'动作冒险',10762:'儿童',10763:'新闻',10764:'真人秀',10765:'科幻奇幻',10766:'肥皂剧',10767:'脱口秀',10768:'战争政治'
};
function normalizeGenreLabel(value){
  const raw = String(value || '').trim();
  const match = raw.match(/^TMDB:(\d+)$/i);
  if(match) return TMDB_GENRE_LABELS[match[1]] || '其他';
  const englishLabels = {
    action:'动作', adventure:'冒险', animation:'动画', comedy:'喜剧', crime:'犯罪', documentary:'纪录片',
    drama:'剧情', family:'家庭', fantasy:'奇幻', history:'历史', horror:'恐怖', music:'音乐', mystery:'悬疑',
    romance:'爱情', 'science-fiction':'科幻', 'science fiction':'科幻', thriller:'惊悚', war:'战争', western:'西部',
    reality:'真人秀', supernatural:'超自然', anime:'动画', children:'儿童'
  };
  return englishLabels[raw.toLowerCase()] || raw;
}
const ADULT_GENRE_BLOCKLIST = new Set([
  '成人','口交','乳交','中出','内射','內射','颜射','顏射','无码','無碼','有码','有碼',
  '巨乳','美乳','人妻','熟女','乱伦','亂倫','乱交','亂交','萝莉','蘿莉','萝莉塔','蘿莉塔',
  '痴女','凌辱','潮吹','自慰','束缚','束縛','露出','素人','里番','裏番','多p','sm',
  '恋乳癖','戀乳癖','单体作品','單體作品','高中女生','女教师','女教師'
]);
function isAdultGenreLabel(value){
  const label = String(value || '').trim().toLowerCase();
  return ADULT_GENRE_BLOCKLIST.has(label) || ADULT_GENRE_BLOCKLIST.has(String(value || '').trim());
}
function isLibraryAdultRecord(media, payload, streamhubCard, genres, contentRating){
  if(payload?.adultManual === true && payload?.adultManualSource === 'user') return Boolean(payload.adult);
  if(payload?.adult || payload?.isAdult || streamhubCard?.adult || streamhubCard?.isAdult) return true;
  if(String(payload?.scrapedBy || '').toLowerCase() === 'jav' || (payload?.jav && typeof payload.jav === 'object')) return true;
  if(/(?:18\s*\+|nc[- ]?17|xxx|nsfw)/i.test(String(contentRating || ''))) return true;
  if((Array.isArray(genres) ? genres : []).some(isAdultGenreLabel)) return true;
  return false;
}
function normalizeGenres(values, fallback = '未分类'){
  const list = (Array.isArray(values) ? values : [values]).map(normalizeGenreLabel).filter(value => value && !/^(video|movie|episode|show|series|本地媒体|本机媒体|local)$/i.test(value));
  const unique = [...new Set(list)];
  return unique.length ? unique : [fallback];
}
function mediaFromBackend(media){
  const payload = media.payload && typeof media.payload === 'object' ? media.payload : {};
  const metadata = payload.metadata && typeof payload.metadata === 'object' ? payload.metadata : {};
  const streamhubCard = metadata.streamhub && typeof metadata.streamhub === 'object' ? metadata.streamhub : metadata;
  const embeddedTitle = streamhubCard.vod_name || streamhubCard.vodName || streamhubCard.name || '';
  const embeddedYear = streamhubCard.vod_year || streamhubCard.vodYear || '';
  const embeddedSummary = streamhubCard.vod_content || streamhubCard.vodContent || streamhubCard.content || '';
  const embeddedGenres = streamhubCard.vod_class || streamhubCard.vodClass || '';
  const providerId = payload.providerId || metadata.providerId || '';
  const providerMediaId = payload.fileId || payload.mediaId || metadata.mediaId || '';
  const externalId = String(payload.externalId || '').trim();
  const openlistStorageId = payload.storageId || metadata.storageId || '';
  const openlistPath = payload.path || media.remotePath || '';
  const genres = normalizeGenres(Array.isArray(payload.genres) && payload.genres.length ? payload.genres : (Array.isArray(streamhubCard.genres) && streamhubCard.genres.length ? streamhubCard.genres : (Array.isArray(streamhubCard.genreLabels) && streamhubCard.genreLabels.length ? streamhubCard.genreLabels : (embeddedGenres ? String(embeddedGenres).split(/[,，|/]/).map(value => value.trim()).filter(Boolean) : [payload.genre || streamhubCard.genre || '未分类']))));
  const contentRating = String(payload.contentRating || streamhubCard.contentRating || '').trim();
  const adult = isLibraryAdultRecord(media, payload, streamhubCard, genres, contentRating);
  // Keep the card cover independent from the home hero backdrop. A generated
  // wide poster must never change the card's original portrait ratio/style.
  const rawArt = media.artUrl || streamhubCard.posterPath || streamhubCard.vod_pic || streamhubCard.vodPic || '';
  const art = normalizeArtworkUrl(rawArt);
  // `backdropUrl` is intentionally kept separate from the card cover. The
  // home hero uses it as a wide/high-resolution poster when available.
  const homePoster = normalizeArtworkUrl(media.backdropUrl || '', '');
  // Remote poster kept as a fallback when the locally-cached cover fails to load.
  const javCard = payload.jav && typeof payload.jav === 'object' ? payload.jav : {};
  const artRemoteRaw = String(payload.artUrlRemote || javCard.coverUrl || '').trim();
  const artRemote = artRemoteRaw ? normalizeArtworkUrl(artRemoteRaw) : '';
  const legacyDarkPreview = /^data:image\//i.test(rawArt) && String(rawArt).length < 10000;
  const scrapedBy = payload.scrapedBy || payload.metadataSource || streamhubCard.scrapedBy || streamhubCard.metadataSource || '';
  const clearArtwork = Boolean(rawArt) && !legacyDarkPreview && !/^assets\/(detail-poster|hero-backdrop)\.(png|jpg)$/i.test(String(rawArt)) && !/^data:image\//i.test(String(rawArt));
  const displayTitle = payload.scrapedBy ? (media.title || embeddedTitle || '未命名媒体') : cleanMediaDisplayTitle(payload.sourceTitle || embeddedTitle || media.title);
  const sourceTitle = payload.sourceTitle || media.remotePath || media.title || '';
  const rawMediaType = payload.mediaType || metadata.mediaType || streamhubCard.mediaType || '';
  const movie = {
    id: media.id,
    record: media,
    metadata: streamhubCard,
    libraryId: media.libraryId || '',
    hasArtwork: clearArtwork,
    artRemote,
    scrapedBy,
    metadataSource: scrapedBy,
    providerId: providerId || undefined,
    providerMediaId: providerMediaId || undefined,
    openlistStorageId: openlistStorageId || undefined,
    openlistPath: openlistPath || undefined,
    sourceTitle,
    episodeIdentity: parseLibraryEpisodeIdentity(media, payload, sourceTitle),
    t: adult ? toSimplifiedZh(displayTitle) : displayTitle,
    tag: '本地媒体库',
    q: detectMediaQualityLabel(sourceTitle || media.title || media.remotePath) || String(media.kind || 'video').toUpperCase(),
    r: Number(media.rating || javCard.rating || streamhubCard.rating || streamhubCard.vod_score || 0),
    y: media.year || (String(javCard.releaseDate || '').slice(0, 4)) || streamhubCard.year || embeddedYear || (/^(19|20)\d{2}$/.test(String(payload.premiered || '').slice(0, 4)) ? String(payload.premiered).slice(0, 4) : '—'),
    d: formatDuration(resolveMediaDurationSeconds(media, javCard)),
    durationSeconds: resolveMediaDurationSeconds(media, javCard),
    img: art,
    homePoster,
    genre: genres[0],
    genres,
    adult,
    contentRating,
    v: media.sourceType || 'local',
    network: 'TTV 本地媒体库',
    type: rawMediaType || (externalId === '420' || payload.matchedTitle === 'little fox' || payload.matchedTitle === 'littlefox' ? 'series' : (parseEpisodeIdentity(sourceTitle) ? 'series' : media.kind || 'video')),
    status: (media.remotePath || providerId || providerMediaId) ? '可播放' : '仅元数据',
    summary: plainText(payload.summary || javCard.summary || streamhubCard.overview || embeddedSummary || media.originalTitle) || (media.remotePath ? '已从本地目录导入，点击播放将交给桌面播放器。' : providerId ? '已从媒体中心导入，播放时会解析真实资源。' : '该条目没有绑定可播放文件。'),
    playUrl: media.remotePath || '',
    sourceLabel: providerId === 'openlist' ? 'OpenList 云盘' : (providerId === 'streamhub' ? 'StreamHub 本机媒体中心' : (providerId === 'guangya' ? '光鸭云盘' : (media.sourceType === 'local' ? '本地文件' : (librarySourceName(media.sourceType) || media.sourceType || '已连接来源')))),
    versions: Array.isArray(streamhubCard.versions) ? streamhubCard.versions : (Array.isArray(payload.versions) ? payload.versions : [])
  };
  const rawEpisodes = payload.episodes || metadata.episodes || streamhubCard.episodes || media.episodes;
  movie.episodes = normalizeEpisodes(rawEpisodes, movie);
  movie.episodesLoaded = movie.episodes.length > 0 || !isStreamHubShow(movie);
  return movie;
}
function mediaFromProviderFile(file){
  const card = file.metadata?.streamhub || file.metadata || {};
  const genres = normalizeGenres(Array.isArray(card.genreLabels) && card.genreLabels.length ? card.genreLabels : [card.genre || '本机媒体'], '未分类');
  const adult = Boolean(file.adult || file.isAdult || card.adult || card.isAdult || /(?:色情|成人|porn|hentai|nsfw)/i.test(genres.join(' ')));
  const movie = {
    id: 'streamhub:' + file.id,
    metadata: card,
    providerId: 'streamhub',
    providerMediaId: file.id,
    t: adult ? toSimplifiedZh(file.name || '未命名媒体') : (file.name || '未命名媒体'),
    tag: 'StreamHub 媒体中心',
    q: String(card.mediaType || 'video').toUpperCase(),
    r: Number(card.rating || 0),
    y: card.year || '—',
    d: formatDuration(Number(file.durationSeconds || card.durationSeconds || 0)),
    durationSeconds: Number(file.durationSeconds || card.durationSeconds || 0),
    img: normalizeArtworkUrl(file.thumbnailUrl),
    homePoster: normalizeArtworkUrl(file.homePosterUrl || card.backdropPath || '', ''),
    hasArtwork: Boolean((file.thumbnailUrl && !isEphemeralArtworkUrl(file.thumbnailUrl)) || card.posterPath),
    genre: genres[0],
    genres,
    adult,
    v: card.contentCategory || '本机媒体',
    network: 'StreamHub',
    type: card.mediaType || (file.name && parseEpisodeIdentity(file.name) ? 'series' : 'video'),
    status: '可播放',
    summary: card.overview || '来自已连接 StreamHub 媒体中心的真实媒体条目。',
    sourceLabel: 'StreamHub 本机媒体中心',
    versions: Array.isArray(card.versions) ? card.versions : []
  };
  movie.episodes = normalizeEpisodes(card.episodes, movie);
  movie.episodesLoaded = movie.episodes.length > 0 || !isStreamHubShow(movie);
  return movie;
}
function librarySeriesKey(movie, identity){
  const source = movie.providerId || movie.v || movie.sourceLabel || 'local';
  const library = movie.libraryId || 'default';
  return [source, library, identity.seriesTitle.toLocaleLowerCase('zh-CN')].join('::');
}
function episodeFromLibraryMovie(movie, identity){
  const label = identity.episodeTitle || (identity.episodeNumber ? `第 ${String(identity.episodeNumber).padStart(2, '0')} 集` : movie.t);
  return {
    id: movie.id,
    providerId: movie.providerId,
    providerMediaId: movie.providerMediaId,
    playUrl: movie.playUrl || '',
    playHeaders: movie.playHeaders || {},
    title: label,
    seasonNumber: identity.seasonNumber || 1,
    episodeNumber: identity.episodeNumber || 0,
    durationSeconds: movie.durationSeconds || 0,
    durationLabel: movie.d,
    img: movie.img,
    summary: movie.summary,
    sourceTitle: movie.sourceTitle,
    raw: movie.record || movie
  };
}
function movieMergeKeys(movie){
  const payload = movie.record?.payload && typeof movie.record.payload === 'object' ? movie.record.payload : {};
  const provider = String(movie.providerId || movie.v || 'local').toLowerCase();
  const scraped = movie.scrapedBy || payload.scrapedBy || '';
  const externalId = String(payload.externalId || '').trim();
  const matchedTitle = String(payload.matchedTitle || '').trim();
  const keys = [];
  if(scraped && (externalId || matchedTitle)){
    keys.push(['scraped', provider, (externalId || matchedTitle).toLowerCase()].join('::'));
  }
  const title = String(movie.t || '').trim().toLowerCase();
  if(title && title !== '未命名媒体'){
    keys.push(['title', provider, title].join('::'));
  }
  return keys;
}
function movieCopyScore(movie){
  let score = 0;
  if(movie.scrapedBy) score += 4;
  if(movie.hasArtwork && movie.img) score += 2;
  if(movie.playUrl || movie.providerId) score += 1;
  return score;
}
function standaloneCopyLabel(movie, index){
  const source = String(movie.sourceTitle || movie.t || '');
  const stem = source.split(/[\\/]/).pop().replace(/\.(mkv|mp4|avi|mov|m4v|webm|wmv|ts|m2ts|flv|rmvb?)$/i, '');
  const quality = stem.match(/\b(2160p|1080p|1080i|720p|480p|4k|remux|bluray|web-dl|webrip|hdtv|hdr10\+?|dv)\b/i);
  if(quality) return quality[0].toUpperCase();
  const ext = source.match(/\.(\w{2,5})$/);
  if(ext && /^(mkv|mp4|avi|mov|m4v|webm|wmv|ts|m2ts|flv|rmvb?)$/i.test(ext[1])) return ext[1].toUpperCase() + ' 副本';
  return '副本 ' + (index + 1);
}
function mergeStandaloneCopy(group, movie, index){
  const payload = movie.record?.payload && typeof movie.record.payload === 'object' ? movie.record.payload : {};
  const metadata = payload.metadata && typeof payload.metadata === 'object' ? payload.metadata : {};
  if(!Array.isArray(group.versions)) group.versions = [];
  const label = standaloneCopyLabel(movie, group.versions.length + index);
  group.versions.push({
    name: label,
    quality: label,
    fileName: movie.sourceTitle || movie.t || '',
    fileSize: positiveNumber(metadata.fileSize) || undefined,
    durationSeconds: movie.durationSeconds || undefined,
    __media: {
      id: movie.id,
      providerId: movie.providerId,
      providerMediaId: movie.providerMediaId,
      openlistStorageId: movie.openlistStorageId,
      openlistPath: movie.openlistPath,
      playUrl: movie.playUrl || '',
      playHeaders: movie.playHeaders || {},
      record: movie.record
    }
  });
}
function groupLibrarySeries(items){
  if(!Array.isArray(items) || !items.length) return [];
  const output = [];
  const groups = new Map();
  const standaloneIndex = new Map();
  items.forEach(movie => {
    if(Array.isArray(movie.episodes) && movie.episodes.length && !movie.episodeIdentity){
      output.push(movie);
      return;
    }
    const identity = movie.episodeIdentity || parseEpisodeIdentity(movie.sourceTitle || movie.t);
    if(!identity || !identity.episodeNumber){
      // 同一部影片的多个文件只保留一张卡片,其余副本并入"版本"仍可单独播放。
      const keys = movieMergeKeys(movie);
      let group = null;
      for(const key of keys){
        const candidate = standaloneIndex.get(key);
        if(candidate){ group = candidate; break; }
      }
      if(!group){
        output.push(movie);
        keys.forEach(key => standaloneIndex.set(key, movie));
        return;
      }
      const existingVersions = Array.isArray(group.versions) ? group.versions : [];
      if(movieCopyScore(movie) > movieCopyScore(group)){
        const previous = {...group};
        Object.assign(group, movie);
        group.versions = existingVersions;
        mergeStandaloneCopy(group, previous, 0);
      }else{
        group.versions = existingVersions;
        mergeStandaloneCopy(group, movie, 0);
      }
      keys.forEach(key => standaloneIndex.set(key, group));
      return;
    }
    const key = librarySeriesKey(movie, identity);
    let group = groups.get(key);
    if(!group){
      group = {
        ...movie,
        t: identity.seriesTitle,
        seriesTitle: identity.seriesTitle,
        type: 'series',
        q: '电视剧',
        playUrl: '',
        previewPlayUrl: movie.playUrl || '',
        status: '可播放剧集',
        episodeIdentity: null,
        episodes: [],
        episodesLoaded: true,
        seriesRecordIds: [],
        seriesRecords: []
      };
      groups.set(key, group);
      output.push(group);
    }
    group.seriesRecordIds.push(movie.id);
    if(movie.record) group.seriesRecords.push(movie.record);
    group.episodes.push(episodeFromLibraryMovie(movie, identity));
    if(!group.previewPlayUrl && movie.playUrl) group.previewPlayUrl = movie.playUrl;
    if(!group.hasArtwork && movie.hasArtwork){
      group.img = movie.img;
      group.hasArtwork = true;
    }
    if(!group.homePoster && movie.homePoster) group.homePoster = movie.homePoster;
  });
  groups.forEach(group => {
    group.episodes.sort((left, right) => left.seasonNumber - right.seasonNumber || left.episodeNumber - right.episodeNumber || String(left.id).localeCompare(String(right.id)));
    group.d = group.episodes.length + ' 集';
    group.status = group.episodes.length + ' 集可播放';
    group.durationSeconds = group.episodes.reduce((total, episode) => total + positiveNumber(episode.durationSeconds), 0);
  });
  return output;
}
const posterCaptureInFlight = new Set();
const posterHydrationFailures = new Set();
let posterHydrationTimer = null;
let posterHydrationGeneration = 0;
function toVideoPreviewUrl(value){
  const text = String(value || '').trim();
  if(!text) return '';
  if(/^(https?:|blob:|data:)/i.test(text)) return text;
  if(window.TtvConvertFileSrc){
    try{
      const localPath = /^file:\/\//i.test(text)
        ? decodeURIComponent(new URL(text).pathname.replace(/^\/+/, '').replace(/^([A-Za-z]):\//, '$1:/'))
        : text;
      return window.TtvConvertFileSrc(localPath);
    }catch(error){ console.warn('Unable to convert local media path:', error); }
  }
  const normalized = text.replace(/\\/g, '/');
  if(/^[A-Za-z]:\//.test(normalized)) return 'file:///' + encodeURI(normalized);
  return /^file:\/\//i.test(text) ? text : normalized;
}
function inspectVideoPreview(sourceUrl, capturePoster = true, maxWidth = 640){
  return new Promise((resolve, reject) => {
    const video = document.createElement('video');
    const canvas = document.createElement('canvas');
    let settled = false;
    let timeout = null;
    let durationSeconds = 0;
    const finish = (error, value) => {
      if(settled) return;
      settled = true;
      if(timeout) window.clearTimeout(timeout);
      video.removeAttribute('src');
      video.load();
      error ? reject(error) : resolve(value);
    };
    const capture = () => {
      if(!video.videoWidth || !video.videoHeight) return finish(new Error('视频没有可用画面'));
      const width = Math.min(maxWidth, video.videoWidth);
      const height = Math.max(1, Math.round(width * video.videoHeight / video.videoWidth));
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext('2d');
      if(!context) return finish(new Error('无法创建画布'));
      try{
        context.drawImage(video, 0, 0, width, height);
        finish(null, {poster:canvas.toDataURL('image/jpeg', 0.82), durationSeconds});
      }catch(error){
        finish(error);
      }
    };
    timeout = window.setTimeout(() => finish(new Error(capturePoster ? '预览画面读取超时' : '视频时长读取超时')), POSTER_HYDRATION_TIMEOUT_MS);
    video.muted = true;
    video.playsInline = true;
    video.preload = capturePoster ? 'auto' : 'metadata';
    video.crossOrigin = 'anonymous';
    video.onerror = () => finish(new Error('视频源无法解码'));
    video.onloadedmetadata = () => {
      if(Number.isFinite(video.duration) && video.duration > 0){
        durationSeconds = video.duration;
        if(!capturePoster) return finish(null, {poster:'', durationSeconds});
        video.currentTime = Math.min(Math.max(0, video.duration - 1), Math.max(3, video.duration * 0.08));
      }else if(capturePoster){
        capture();
      }else{
        finish(new Error('视频没有可用时长'));
      }
    };
    video.onseeked = capture;
    video.src = toVideoPreviewUrl(sourceUrl);
    video.load();
  });
}
async function resolveMoviePreviewUrl(movie){
  if(movie.previewPlayUrl) return movie.previewPlayUrl;
  if(movie.playUrl) return movie.playUrl;
  if(!movie.providerId || !movie.providerMediaId || !TtvBackend.available()) return '';
  const playback = movie.providerId === 'openlist'
    ? await TtvBackend.invoke('openlist_resolve_playback', {input:{storageId:String(movie.openlistStorageId || movie.storageId || ''), path:String(movie.openlistPath || movie.providerMediaId), mediaId:String(movie.id), quality:null}})
    : await TtvBackend.invoke('provider_resolve_playback', {
      providerId: movie.providerId,
      request: {mediaId: movie.providerMediaId, quality: null}
    });
  movie.playUrl = playback.url;
  movie.playHeaders = playback.headers || {};
  movie.playbackExpiresAt = playback.expiresAt ?? playback.expires_at ?? null;
  return movie.playUrl;
}
function scheduleIdleTask(callback, delay = 120){
  if(typeof window.requestIdleCallback === 'function'){
    return window.requestIdleCallback(callback, {timeout: 1200});
  }
  return window.setTimeout(callback, delay);
}
async function hydrateMissingVideoPosters(items){
  if(!isNativeMediaMode() || !Array.isArray(items)) return;
  // Catalog refreshes can happen several times during startup. Keep one
  // hydration worker alive instead of spawning overlapping video decoders.
  if(posterHydrationTimer || posterCaptureInFlight.size) return;
  const generation = ++posterHydrationGeneration;
  // Browser video probing is only reliable for local files. Resolving cloud
  // playback URLs here caused expiring signed URLs, repeated 404s and startup
  // decoder storms. Remote items keep their provider artwork or placeholder.
  const queue = items.filter(movie => {
    const key = String(movie?.id ?? '');
    const source = movie?.previewPlayUrl || movie?.playUrl || '';
    const needsHomePoster = !movie?.homePoster && !movie?.adult && Boolean(movie?.scrapedBy);
    if(!key
      || posterHydrationFailures.has(key)
      || !isLocalMediaSource(source)
      || (movie.hasArtwork && movie.durationSeconds > 0 && !needsHomePoster)) return false;
    const card = document.querySelector('.movie-card[data-movie-id="' + key.replace(/"/g, '') + '"]');
    if(card) return cardNearViewport(card, 1.2);
    return needsHomePoster && items.indexOf(movie) < 8;
  }).slice(0, POSTER_HYDRATION_LIMIT);
  let cursor = 0;
  const runBatch = async () => {
    if(generation !== posterHydrationGeneration || document.hidden) return;
    let changed = false;
    const end = Math.min(cursor + POSTER_HYDRATION_BATCH_SIZE, queue.length);
    for(; cursor < end; cursor++){
      const movie = queue[cursor];
      if(generation !== posterHydrationGeneration) return;
      const movieKey = String(movie.id);
      const needsPoster = !movie.hasArtwork;
      const needsDuration = !(movie.durationSeconds > 0);
      const needsHomePoster = !movie.homePoster && !movie.adult && Boolean(movie.scrapedBy);
      if((!needsPoster && !needsDuration && !needsHomePoster) || posterCaptureInFlight.has(movieKey)) continue;
      posterCaptureInFlight.add(movieKey);
      try{
        const sourceUrl = await resolveMoviePreviewUrl(movie);
        if(!sourceUrl) continue;
        const preview = await inspectVideoPreview(sourceUrl, needsPoster || needsHomePoster, needsHomePoster ? 1280 : 640);
        if(needsPoster && preview.poster){
          movie.img = preview.poster;
          movie.hasArtwork = true;
          changed = true;
        }
        if(needsHomePoster && preview.poster){
          movie.homePoster = preview.poster;
          changed = true;
        }
        if(preview.durationSeconds > 0){
          movie.durationSeconds = preview.durationSeconds;
          movie.d = formatDuration(preview.durationSeconds);
          changed = true;
        }
        if(needsPoster){
          await TtvBackend.invoke('library_set_preview', {input:{
            mediaId:String(movie.id),
            artUrl:movie.img,
            durationSeconds:preview.durationSeconds > 0 ? Math.round(preview.durationSeconds) : null
          }});
        }else if(preview.durationSeconds > 0){
          await TtvBackend.invoke('library_set_preview', {input:{
            mediaId:String(movie.id),
            artUrl:movie.img || '/assets/detail-poster.jpg',
            durationSeconds:Math.round(preview.durationSeconds)
          }});
        }
        if(needsHomePoster && movie.homePoster){
          await TtvBackend.invoke('library_set_home_poster', {input:{mediaId:String(movie.id), artUrl:movie.homePoster}});
        }
        posterHydrationFailures.delete(movieKey);
        if(selectedMovie && String(selectedMovie.id) === String(movie.id)) syncPlayerContent(movie);
      }catch(error){
        posterHydrationFailures.add(movieKey);
        console.debug('Optional local video preview unavailable:', movie.t, error);
      }finally{
        posterCaptureInFlight.delete(movieKey);
      }
    }
    if(changed && generation === posterHydrationGeneration){
      // Update already-rendered cards in place. Rebuilding the whole grid for
      // every metadata batch caused visible jank and restarted card insertion.
      queue.slice(0, cursor).forEach(movie => {
        const id = String(movie?.id ?? '');
        if(!id) return;
        document.querySelectorAll('.movie-card[data-movie-id]').forEach(card => {
          if(card.dataset.movieId !== id) return;
          card.querySelectorAll('img').forEach(image => {
            if(!movie.img) return;
            image.dataset.coverSrc = movie.img;
            if(image.dataset.coverLoaded === '1' || cardNearViewport(card, 0.4)){
              image.src = movie.img;
              image.classList.remove('is-pending');
              image.dataset.coverLoaded = '1';
            }
          });
          const sub = card.querySelector('.m-sub');
          if(sub && movie.d) sub.textContent = `${movie.r > 0 ? '★' + Number(movie.r).toFixed(1) : '—'} ${movie.y || '—'} · ${movie.d}`;
        });
      });
      if(changed && currentView === 'home' && homeMovies().length){
        renderGallery();
        setHeroImage(imgCur, homeMovieAt(current));
      }
    }
    if(cursor < queue.length && generation === posterHydrationGeneration){
      posterHydrationTimer = scheduleIdleTask(() => {
        posterHydrationTimer = null;
        void runBatch();
      }, 240);
    }
  };
  posterHydrationTimer = scheduleIdleTask(() => {
    posterHydrationTimer = null;
    void runBatch();
  });
}
function isNativeMediaMode(){
  return TtvBackend.available() && appMode !== 'catalog';
}
function updateCatalogChrome(){
  const count = MOVIES.filter(movie => !movie.adult).length;
  const isStreamHub = appMode === 'streamhub';
  const countEl = document.getElementById('libraryCount');
  const resultEl = document.getElementById('libraryResultCount');
  const status = document.getElementById('catalogStatus');
  if(countEl) countEl.textContent = count + ' 条';
  if(resultEl) resultEl.textContent = '共 ' + count + ' 条' + (isStreamHub ? ' StreamHub 媒体' : (appMode === 'desktop' ? '本地媒体' : '公开目录内容'));
  if(status) status.textContent = isStreamHub ? 'StreamHub · 已加载 ' + count + ' 条真实媒体' : (appMode === 'desktop' ? '桌面端 · 已加载 ' + count + ' 条本地媒体' : '公开目录 · 已同步 ' + count + ' 条');
}
function renderYearOptions(){
  const menu = document.getElementById('yearMenu');
  if(!menu) return;
  // 18+ 隔离：年份候选同样只统计常规条目，避免选中后筛出空列表。
  const years = [...new Set(MOVIES.filter(movie => !movie.adult).map(movie => String(movie?.y || '')).filter(year => /^\d{4}$/.test(year)))].sort((a,b) => Number(b) - Number(a));
  menu.innerHTML = '<div class="dd-item" onclick="pickYear(\'年份\')">全部年份</div>' + years.map(year => `<div class="dd-item" onclick="pickYear('${year}')">${year}</div>`).join('');
  if(activeYear !== '年份' && !years.includes(activeYear)) activeYear = '年份';
  const label = document.getElementById('yearLabel');
  if(label) label.textContent = activeYear;
}
function restoreCatalogFavorites(){
  favoriteIds.clear();
  try{
    const stored = JSON.parse(localStorage.getItem(CATALOG_FAVORITES_KEY) || '[]');
    (Array.isArray(stored) ? stored : []).map(String).forEach(id => favoriteIds.add(id));
  }catch(error){ console.warn('Unable to restore catalog favorites:', error); }
}
function isMovieFavorite(movie){
  if(!movie) return false;
  if(favoriteIds.has(String(movie.id))) return true;
  return Array.isArray(movie.seriesRecordIds) && movie.seriesRecordIds.some(id => favoriteIds.has(String(id)));
}
async function refreshUserContent(){
  const generation = ++favoriteLoadGeneration;
  if(!isNativeMediaMode()){
    restoreCatalogFavorites();
    const hours = document.getElementById('myWatchHours');
    const count = document.getElementById('myFavoriteCount');
    const size = document.getElementById('myLibrarySize');
    if(hours) hours.textContent = '—';
    if(count) count.textContent = String(favoriteIds.size);
    if(size) size.innerHTML = '— <small style="font-size:14px">GB</small>';
    renderWatchlist();
    return;
  }
  try{
    const [favorites, stats] = await Promise.all([
      TtvBackend.invoke('favorites_list', {limit: 5000}),
      TtvBackend.invoke('library_stats')
    ]);
    if(generation !== favoriteLoadGeneration) return;
    favoriteIds.clear();
    (Array.isArray(favorites) ? favorites : []).forEach(item => favoriteIds.add(String(item.id)));
    const hours = document.getElementById('myWatchHours');
    const count = document.getElementById('myFavoriteCount');
    const size = document.getElementById('myLibrarySize');
    if(hours) hours.innerHTML = `${(Number(stats?.watchedSeconds || 0) / 3600).toFixed(1)} <small style="font-size:14px;font-weight:600">小时</small>`;
    if(count) count.textContent = String(Number(stats?.favoriteCount || favoriteIds.size));
    if(size) size.innerHTML = stats?.storageBytes == null ? '— <small style="font-size:14px">GB</small>' : `${(Number(stats.storageBytes) / 1073741824).toFixed(1)} <small style="font-size:14px">GB</small>`;
    renderWatchlist();
    syncFavoriteButtons();
  }catch(error){
    if(generation !== favoriteLoadGeneration) return;
    console.warn('Unable to load favorites and library stats:', error);
    const count = document.getElementById('myFavoriteCount');
    if(count) count.textContent = '—';
    renderWatchlist();
  }
}
function syncFavoriteButtons(){
  document.querySelectorAll('.movie-card[data-movie-id]').forEach(card => {
    const movie = MOVIES.find(item => String(item?.id) === card.dataset.movieId);
    const button = card.querySelector('[data-act="fav"]');
    if(button && movie) button.setAttribute('aria-pressed', String(isMovieFavorite(movie)));
  });
}
function updateHeroHomeState(){
  const tag = heroCopy?.querySelector('.hero-recommend-tag');
  const badge = heroCopy?.querySelector('.marvel-badge');
  const title = document.getElementById('heroTitle');
  const desc = document.getElementById('heroDesc');
  const playLabel = document.getElementById('heroPlayLabel');
  const hasMedia = MOVIES.length > 0;
  const hasHomeMedia = homeMovies().length > 0;
  if(hasMedia && hasHomeMedia){
    if(playLabel) playLabel.textContent = '立即播放';
    return;
  }
  if(hasMedia && appMode === 'desktop'){
    if(tag) tag.textContent = '媒体库状态';
    if(badge) badge.textContent = '本地媒体库';
    if(title) title.textContent = hasMedia + ' 条媒体可播放，等待刮削';
    if(desc) desc.textContent = '视频文件已导入，但缺少海报或简介。刮削完成后首页会展示可浏览的封面轮播。';
    if(playLabel) playLabel.textContent = '播放首个媒体';
    return;
  }
  if(hasMedia){
    if(tag) tag.textContent = '目录状态';
    if(badge) badge.textContent = '公开目录';
    if(title) title.textContent = '已同步部分媒体';
    if(desc) desc.textContent = '部分公开目录缺少封面，先显示可播放条目；连接本地媒体库后可获得完整海报墙。';
  }else{
    if(tag) tag.textContent = '媒体库状态';
    if(badge) badge.textContent = '等待添加来源';
    if(title) title.textContent = appMode === 'catalog' ? '暂无可显示的视频' : '媒体库为空';
    if(desc) desc.textContent = appMode === 'catalog' ? '公开目录加载失败。请检查网络，或添加本地目录、云盘来源后刷新首页。' : '请先在本地目录或云盘目录配置中选择要扫描的文件夹。';
  }
  if(playLabel) playLabel.textContent = hasMedia ? '播放首个媒体' : '暂无可播放';
}
function applyCatalog(items, mode, options = {}){
  const loading = document.getElementById('catalogLoading');
  if(loading) loading.classList.remove('active');
  MOVIES = mode === 'catalog' ? (Array.isArray(items) ? items : []) : groupLibrarySeries(Array.isArray(items) ? items : []);
  // 全量库落地后才算就绪：首批子集（options.dataReady === false）不算，
  // 否则深夜档会在 18+ 条目尚未进入内存时渲染假 0 统计。
  if(mode !== 'catalog' && options.dataReady !== false) libraryDataReady = true;
  current = 0;
  // 后台刮削会反复 applyCatalog。播放中不能把 selectedMovie 冲回片库第一项，
  // 否则字幕搜索/画质切换读到错误条目，表现为空结果或切画质无效。
  selectedMovie = retainCatalogSelection(selectedMovie, MOVIES) || MOVIES[0] || null;
  detailMovie = retainCatalogSelection(detailMovie, MOVIES) || selectedMovie;
  appMode = mode;
  renderYearOptions();
  renderLibraryCategories();
  if(!MOVIES.length){
    renderGallery();
    setHeroImage(imgCur, null);
    setHeroImage(imgNext, null);
    updateCatalogChrome();
    updateHeroHomeState();
    renderWatchlist();
    if(currentView === 'adult') renderAdultZone();
    void refreshUserContent();
    return;
  }
  applyHeroCopy(current);
  setHeroImage(imgCur, homeMovieAt(current));
  setHeroImage(imgNext, homeMovieAt(current));
  renderGallery();
  renderGrid();
  renderWatchlist();
  if(currentView === 'adult') renderAdultZone();
  scheduleIdleTask(() => { void renderContinueWatching(); }, 700);
  void refreshUserContent();
  restartLiveBar(current);
  updateCatalogChrome();
  updateHeroHomeState();
  void hydrateMissingVideoPosters(MOVIES);
}
function retainCatalogSelection(current, items){
  if(!current?.id || !Array.isArray(items) || !items.length) return null;
  const id = String(current.id);
  const providerMediaId = String(current.providerMediaId || '');
  return items.find(item => {
    if(String(item.id) === id) return true;
    if(providerMediaId && String(item.providerMediaId || '') === providerMediaId) return true;
    const versions = Array.isArray(item.versions) ? item.versions : [];
    if(versions.some(version => String(version?.__media?.id || '') === id)) return true;
    const episodes = Array.isArray(item.episodes) ? item.episodes : [];
    return episodes.some(episode => String(episode?.id || '') === id || (providerMediaId && String(episode?.providerMediaId || '') === providerMediaId));
  }) || (document.body.classList.contains('player-active') ? current : null);
}
async function restoreDesktopSettings(){
  if(!TtvBackend.available()) return;
  try{
    const accent = await TtvBackend.invoke('settings_get', {key: 'appearance.accent'});
    if(!accent) return;
    document.documentElement.style.setProperty('--accent', accent);
    document.documentElement.style.setProperty('--accent-2', accent);
    document.querySelectorAll('.theme-dot').forEach(dot => dot.classList.toggle('active', dot.style.background === accent));
  }catch(error){
    console.warn('Unable to restore settings:', error);
  }
}
async function loadInitialCatalog(){
  if(!TtvBackend.available()) return refreshCatalog();
  const generation = ++libraryLoadGeneration;
  try{ await TtvBackend.invoke('library_repair_adult_isolation'); }catch(error){ console.warn('adult isolation repair skipped', error); }
  const status = document.getElementById('catalogStatus');
  const countCap = document.getElementById('libraryCountCap');
  if(status) status.textContent = '桌面端 · 正在连接本地媒体库';
  try{
    const runtime = await TtvBackend.invoke('runtime_status');
    const entries = [];
    // 首页先小批量渲染（秒开），全量分页用大块（后端上限 5000）：页数从
    // 200/页×142 次串行 IPC 降到 2000/页×15 次，"先显示一小部分再卡住"
    // 的等待窗口从十几秒缩到一秒上下。
    const firstPageSize = 200;
    const pageSize = 2000;
    const firstPage = await TtvBackend.invoke('library_page', {input: {limit: firstPageSize, offset: 0}});
    if(Array.isArray(firstPage) && firstPage.length){
      entries.push(...firstPage);
      const firstMovies = groupLibrarySeries(entries.map(mediaFromBackend));
      if(firstMovies.length){
        const firstMode = firstMovies.some(movie => movie.providerId === 'streamhub') ? 'streamhub' : 'desktop';
        // 首批只是子集：18+ 条目分散在库的后段，此时深夜档统计还不可信，
        // 保持 libraryDataReady=false 让深夜档显示加载占位而不是假 0。
        applyCatalog(firstMovies, firstMode, {dataReady: false});
        if(countCap) countCap.textContent = '正在加载完整媒体库...';
      }
    }
    // 分页拉取整个媒体库。旧上限 5000 会在大库(>1.5万条)里截断,同一排序
    // 区间的同名系列被整批吸入后又因缺封面被并卡,表现为"满屏同一张卡片"。
    for(let offset = entries.length; offset < 200000; offset += pageSize){
      const page = await TtvBackend.invoke('library_page', {input: {limit: pageSize, offset}});
      if(generation !== libraryLoadGeneration) return;
      if(!Array.isArray(page) || !page.length) break;
      entries.push(...page);
      if(countCap) countCap.textContent = `正在加载完整媒体库（${entries.length} 条）...`;
      if(page.length < pageSize) break;
    }
    if(generation !== libraryLoadGeneration) return;
    restoreDesktopSettings();
    const localMovies = Array.isArray(entries) ? groupLibrarySeries(entries.map(mediaFromBackend)) : [];
    if(localMovies.length){
      const mode = localMovies.some(movie => movie.providerId === 'streamhub') ? 'streamhub' : 'desktop';
      // The grouped series count can stay unchanged while a later page adds or
      // replaces episodes, so compare stable media ids instead of only length.
      const currentIds = new Set(MOVIES.map(movie => String(movie?.id ?? '')));
      const nextIds = new Set(localMovies.map(movie => String(movie?.id ?? '')));
      const catalogChanged = currentIds.size !== nextIds.size || [...nextIds].some(id => !currentIds.has(id));
      if(catalogChanged || entries.length > firstPageSize) applyCatalog(localMovies, mode);
      if(countCap) countCap.textContent = '本地媒体库';
      const runtimeHint = runtime?.playbackAvailable === false ? '；播放内核不可用' : '';
      if(status) status.textContent = '桌面端 · 已加载 ' + localMovies.length + ' 条本地媒体' + runtimeHint;
      return;
    }
    if(countCap) countCap.textContent = '本地媒体库为空';
    if(status) status.textContent = '本地媒体库为空 · 扫描并刮削后才会显示首页封面';
    applyCatalog([], 'desktop');
    return;
  }catch(error){
    console.warn('Tauri library load fallback:', error);
    if(countCap) countCap.textContent = '桌面端连接失败';
    if(status) status.textContent = '桌面端连接失败 · 正在加载公开目录预览';
    const loading = document.getElementById('catalogLoading');
    if(loading) loading.classList.remove('active');
  }
  return refreshCatalog();
}
async function refreshProviderStatus(){
  if(!TtvBackend.available()) return;
    const cloudStatus = document.getElementById('cloudScanStatus');
  try{
    const [providers, capabilities, catalog] = await Promise.all([
      TtvBackend.invoke('provider_list'),
      TtvBackend.invoke('provider_capabilities', {providerId: 'guangya'}),
      TtvBackend.invoke('source_catalog')
    ]);
    SOURCE_CATALOG = Array.isArray(catalog) ? catalog : SOURCE_CATALOG;
    const implemented = SOURCE_CATALOG.filter(item => item.implemented).length;
    if(cloudStatus) cloudStatus.textContent = capabilities.browseFiles ? `可浏览目录 · ${implemented} 类来源已接入` : '当前版本不支持浏览文件';
  }catch(error){
    console.warn('Unable to read provider capabilities:', error);
  }
}
async function loadStreamHubResources(){
  if(!TtvBackend.available()){
    toast('StreamHub 资源仅可在 TTV 桌面端读取。');
    return;
  }
  const status = document.getElementById('catalogStatus');
  if(status) status.textContent = 'StreamHub · 正在读取媒体库';
  try{
    const runtime = await TtvBackend.invoke('streamhub_status').catch(() => null);
    let health = await TtvBackend.invoke('streamhub_health').catch(() => null);
    if(!health?.reachable && runtime?.configured && !runtime?.running){
      await TtvBackend.invoke('streamhub_start');
      for(let attempt = 0; attempt < 12 && !health?.reachable; attempt++){
        await new Promise(resolve => setTimeout(resolve, 500));
        health = await TtvBackend.invoke('streamhub_health').catch(() => null);
      }
    }
    if(!health?.reachable){
      throw new Error(health?.message || runtime?.message || 'StreamHub 未启动或健康检查未通过');
    }
    const probe = await TtvBackend.invoke('provider_test_connection', {providerId:'streamhub'});
    const report = await TtvBackend.invoke('provider_sync_library_recursive', {
      providerId: 'streamhub',
      input: {pageSize: 100, maxItems: 100000}
    });
    const fetched = Number(report?.fetched || probe?.itemCount || 0);
    const imported = Number(report?.imported || 0);
    const skipped = Number(report?.skipped || 0);
    if(!fetched){
      toast('StreamHub 已连接，但当前没有可浏览媒体。');
      if(status) status.textContent = 'StreamHub · 媒体库为空';
      return;
    }
    const scraped = await scrapeLibraryUntilDone(5000);
    await loadInitialCatalog();
    toast('已从 StreamHub 拉取 ' + imported + ' 条真实媒体到本地媒体库。' + (skipped ? ' 已跳过 ' + skipped + ' 项。' : '') + scrapeSummary(scraped) + (report?.truncated ? ' 已达到同步上限。' : ''));
  }catch(error){
    const message = backendErrorMessage(error);
    if(status) status.textContent = 'StreamHub · 无法连接';
    toast('无法读取 StreamHub：' + message);
  }
}

function plainText(value){
  const node = document.createElement('div');
  node.innerHTML = value || '';
  return (node.textContent || node.innerText || '').replace(/\s+/g, ' ').trim();
}
function showFromCatalog(show){
  const year = Number((show.premiered || '').slice(0, 4)) || '—';
  const runtime = show.averageRuntime || show.runtime;
  const network = show.network?.name || show.webChannel?.name || 'TVMaze';
  const genres = normalizeGenres(Array.isArray(show.genres) && show.genres.length ? show.genres : ['剧情'], '剧情');
  const movie = {
    id: show.id,
    t: show.name,
    tag: '精选影视',
    q: show.type === 'Scripted' ? '电视剧' : (show.type === 'Reality' ? '真人秀' : '影视'),
    r: Number(show.rating?.average || 0),
    y: year,
    d: runtime ? `${runtime} 分钟/集` : '时长未知',
    img: show.image?.original || show.image?.medium || '/assets/detail-poster.jpg',
    homePoster: show.image?.original || show.image?.medium || '',
    genre: genres[0],
    genres,
    v: show.status === 'Running' ? '连载中' : (show.status === 'Ended' ? '已完结' : '状态未知'),
    network,
    type: show.type === 'Scripted' ? 'series' : 'video',
    status: show.status === 'Running' ? '连载中' : (show.status === 'Ended' ? '已完结' : '状态未知'),
    summary: plainText(show.summary) || '公开目录暂未提供剧情简介。',
    sourceLabel: 'TVMaze 公开目录',
    catalogSourceId: show.id
  };
  try{
    const override = JSON.parse(localStorage.getItem('ttv.catalogOverride.' + show.id) || 'null');
    if(override && typeof override === 'object') Object.assign(movie, override);
  }catch(error){ console.warn('Unable to restore catalog override:', error); }
  return movie;
}
async function refreshCatalog(){
  const status = document.getElementById('catalogStatus');
  const loading = document.getElementById('catalogLoading');
  if(status) status.textContent = '公开目录 · 正在同步';
  const loadingFallback = setTimeout(() => { if(loading) loading.classList.remove('active'); }, 3500);
  try{
    // 单条失败不拖垮整批：能拿几条用几条，凑不满 4 条才判定同步失败。
    const settled = await Promise.allSettled(CATALOG_IDS.map(id => fetch(TVMAZE_API + id, {cache:'no-store'}).then(async response => {
      if(!response.ok) throw new Error('HTTP ' + response.status + ' for show ' + id);
      return showFromCatalog(await response.json());
    })));
    const synced = settled.flatMap(result => result.status === 'fulfilled' ? [result.value] : []).filter(item => item && item.img);
    if(synced.length < 4) throw new Error('Catalog response was incomplete');
    applyCatalog(synced, 'catalog');
  }catch(error){
    appMode = 'catalog';
    updateCatalogChrome();
    updateHeroHomeState();
    if(status) status.textContent = '公开目录 · 无法加载真实数据';
    // 离线/断网属预期场景，仅记 message，不再打印带堆栈的完整 error。
    console.warn('TVMaze catalog sync fallback:', error && error.message);
  }finally{
    clearTimeout(loadingFallback);
    if(loading) loading.classList.remove('active');
  }
}

/* ================= 轮播头部交互 ================= */
function setSourceTab(btn, type){
  btn.parentElement.querySelectorAll('.hg-src-btn').forEach(b => b.classList.remove('active'));
  btn.classList.add('active');
  if(type === 'cloud') loadStreamHubResources();
  else loadInitialCatalog();
}
function setCategoryTab(btn, cat){
  btn.parentElement.querySelectorAll('.hg-cat-tab').forEach(b => b.classList.remove('active'));
  btn.classList.add('active');
  toast(cat === 'latest' ? '已筛选：最新影视' : '已筛选：最近更新');
}

/* ================= 路由 ================= */
function syncHomeViewportHeight(){
  const viewportHeight = window.visualViewport?.height || window.innerHeight;
  document.documentElement.style.setProperty('--home-viewport-height', Math.max(1, viewportHeight).toFixed(3) + 'px');
}
function syncViewDocumentState(name){
  const homeActive = name === 'home';
  document.documentElement.classList.toggle('home-active', homeActive);
  document.body.classList.toggle('home-active', homeActive);
  document.body.classList.toggle('cloud-browser-active', name === 'cloud-browser');
}
syncHomeViewportHeight();
window.addEventListener('resize', syncHomeViewportHeight, {passive:true});
window.visualViewport?.addEventListener('resize', syncHomeViewportHeight, {passive:true});
let currentView = 'home';
function showView(name, syncHash = true){
  if(name === 'my-content') name = 'profile';
  const target = document.getElementById('view-' + name);
  if(!target) return;
  // 播放器是覆盖层，不参与 currentView 的常规路由状态。导航、哈希路由或
  // 异常恢复把页面带回其它视图时，先显式销毁原生 actor/HTML5 媒体，避免
  // 画面回到首页但旧音频仍在后台继续播放。
  if(name !== 'player'){
    if(player?.classList.contains('active')) closePlayer();
    else ++playerSessionId;
  }
  syncViewDocumentState(name);
  if(currentView === name){
    window.scrollTo({top:0, behavior:'instant'});
    if(name === 'cloud-browser') renderCloudBrowser();
    if(name === 'profile'){
      void refreshUserContent();
      void renderContinueWatching();
    }
    if(name === 'logs') void refreshRuntimeDiagnostics();
    if(name === 'short-drama') shortDramaEnsureStarted();
    return;
  }
  document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
  target.classList.add('active');
  document.querySelectorAll('.nav-tab').forEach(t => t.classList.toggle('active', t.dataset.view === name));
  document.querySelector('.user-pill')?.classList.toggle('active', name === 'profile');
  currentView = name;
  if(syncHash){
    // 深夜档是隐藏页面：永远不写入 location.hash，避免历史记录泄露
    const nextHash = (name === 'home' || name === 'adult') ? '' : '#' + name;
    history.replaceState(null, '', location.pathname + location.search + nextHash);
  }
  window.scrollTo({top:0, behavior:'instant'});
  stagger(target);
  if(name === 'cloud' && !activeCloudSource) selectCloudSource('本地磁盘');
  if(name === 'cloud-browser') renderCloudBrowser();
  if(name === 'profile'){
    void refreshUserContent();
    void renderContinueWatching();
  }
  if(name === 'logs') void refreshRuntimeDiagnostics();
  if(name === 'settings') refreshTmdbStatusCard();
  if(name === 'adult') renderAdultZone();
  if(name === 'short-drama') shortDramaEnsureStarted();
  // 通过常规导航离开深夜档时，静默清理夜间主题
  if(name !== 'adult' && document.body.classList.contains('adult-mode')){
    document.body.classList.remove('adult-mode');
  }
}
document.querySelectorAll('.nav-tab').forEach(t => t.addEventListener('click', () => showView(t.dataset.view)));

/* ============ 文本与入场动效引擎 ============ */
function renderWordPullUp(container, text){
  if(!container) return;
  container.textContent = text || '';
}

/* 交错入场动画 */
function stagger(root){
  if(!root) return;
  root.querySelectorAll('[data-stagger]').forEach((el, i) => {
    el.classList.remove('rise-in');
    el.style.animationDelay = '';
    void el.offsetWidth;
    el.style.animationDelay = (i * 70) + 'ms';
    el.classList.add('rise-in');
  });
}

/* ================= 首页 · 大封面与画廊联动轮播 ================= */
const heroEl = document.getElementById('hero');
const heroCopy = document.getElementById('heroCopy');
const galleryBox = document.querySelector('.hero-gallery');
const HERO_HOLD = 6000;      // 每页停留 ms
const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const HERO_TRANSITION_MS = reducedMotion ? 180 : 620;
const HERO_IMAGE_TIMEOUT_MS = 2600;
heroEl.style.setProperty('--hero-transition-duration', HERO_TRANSITION_MS + 'ms');
const qs = location.search;
const morphhold = qs.indexOf('morphhold') !== -1;
const slideParam = qs.match(/slide=(\d+)/);
const startIdx = slideParam ? Math.max(0, parseInt(slideParam[1], 10) || 0) : 0;

function isHomeEligible(movie){
  if(!movie || isUnsafeHomeMedia(movie)) return false;
  const payload = movie.record?.payload;
  const scraped = Boolean(movie.scrapedBy || movie.metadataSource || payload?.scrapedBy || payload?.metadataSource || movie.metadata?.scrapedBy || movie.metadata?.metadataSource);
  return scraped && movie.hasArtwork && Boolean(movie.img) && !/^assets\/(detail-poster|hero-backdrop)\.(png|jpg)$/i.test(String(movie.img));
}
function isUnsafeHomeMedia(movie){
  if(!movie) return true;
  const payload = movie.record?.payload && typeof movie.record.payload === 'object' ? movie.record.payload : {};
  const metadata = movie.metadata && typeof movie.metadata === 'object' ? movie.metadata : {};
  const rating = String(movie.contentRating || payload.contentRating || metadata.contentRating || '').trim();
  if(movie.adult || payload.adult || payload.isAdult || (payload.adultManual === true && payload.adult === true) || metadata.adult || metadata.isAdult) return true;
  if(/(?:18\s*\+|nc[- ]?17|xxx|nsfw)/i.test(rating)) return true;
  if(payload.jav || metadata.jav || payload.adultMetadata) return true;
  const labels = [movie.genre, ...(Array.isArray(movie.genres) ? movie.genres : []), ...(Array.isArray(payload.genres) ? payload.genres : [])].join(' ');
  if(/(?:色情|成人|无码|有码|porn|hentai|xxx|nsfw|adult)/i.test(labels)) return true;
  // Conservative title/path fallback for records whose backend classifier has
  // not run yet. Require an explicit marker or a known JAV-style code shape so
  // ordinary names such as “Level 03” stay visible.
  const source = String(movie.sourceTitle || movie.t || '').trim();
  return /(?:色情|成人影片|无码|有码|porn|hentai|nsfw|\b(?:JAV|FC2(?:[-_ ]?PPV)?|IPX|SSIS|MIDE|ABP|ADN|FSDSS)[-_ ]?\d{3,8}\b)/i.test(source);
}
function homeMovies(){ return MOVIES.filter(isHomeEligible); }
function homeMovieAt(index){
  const items = homeMovies();
  return items.length ? items[((index % items.length) + items.length) % items.length] : null;
}

function heroPosterUrl(movie){
  return normalizeArtworkUrl(movie?.homePoster || movie?.img || '', '/assets/hero-backdrop.jpg');
}
function setHeroImage(image, movie){
  if(!image) return;
  image.__heroMovie = movie || null;
  image.dataset.heroCardTried = '0';
  image.dataset.heroRemoteTried = '0';
  if(image.dataset.heroBound !== '1'){
    image.dataset.heroBound = '1';
    image.addEventListener('error', () => {
      const currentMovie = image.__heroMovie;
      if(currentMovie?.img && image.dataset.heroCardTried !== '1' && image.getAttribute('src') !== currentMovie.img){
        image.dataset.heroCardTried = '1';
        image.src = normalizeArtworkUrl(currentMovie.img, '/assets/hero-backdrop.jpg');
        return;
      }
      // This is only an already-supplied metadata URL from the existing
      // record. The homepage does not call a new poster service or fetch one.
      if(currentMovie?.artRemote && image.dataset.heroRemoteTried !== '1'){
        image.dataset.heroRemoteTried = '1';
        image.src = normalizeArtworkUrl(currentMovie.artRemote, '/assets/hero-backdrop.jpg');
        return;
      }
      if(image.getAttribute('src') !== '/assets/hero-backdrop.jpg') image.src = '/assets/hero-backdrop.jpg';
    });
  }
  image.alt = movie?.t ? `${movie.t} 首页海报` : '首页海报';
  image.src = heroPosterUrl(movie);
}

/* ---- 文案随轮播切换 ---- */
function applyHeroCopy(i){
  const m = homeMovieAt(i);
  if(!m) return;
  const tagEl = heroCopy.querySelector('.hero-recommend-tag');
  if(tagEl) tagEl.textContent = m.tag || '首映推荐';
  const studioEl = heroCopy.querySelector('.hero-studio-tag');
  if(studioEl){
    studioEl.innerHTML = `<span class="marvel-badge">${escapeHtml(m.studio || '影视推荐')}</span>`;
  }
  const titleEl = heroCopy.querySelector('.hero-title');
  if(titleEl){
    titleEl.classList.toggle('is-long', m.t.length > 24 && m.t.length <= 46);
    titleEl.classList.toggle('is-xlong', m.t.length > 46);
    if(m.t.includes('：') || m.t.includes(':')){
      const parts = m.t.split(/[:：]/);
      titleEl.innerHTML = `${escapeHtml(parts[0])}<span class="hero-title-sub">${escapeHtml(parts.slice(1).join('：'))}</span>`;
    } else {
      titleEl.textContent = m.t;
    }
  }
  const descEl = heroCopy.querySelector('.hero-desc');
  if(descEl) descEl.textContent = m.summary || '';
}
function swapHeroCopy(i){
  if(reducedMotion){ applyHeroCopy(i); return; }
  heroCopy.classList.add('out');
  setTimeout(() => {
    applyHeroCopy(i);
    requestAnimationFrame(() => {
      heroCopy.classList.remove('out');
    });
  }, 200);
}

/* ---- 状态与联动 ---- */
let current = startIdx, animating = false, pending = null, hoverPause = false, holdTimer = null;
applyHeroCopy(current);

/* ---- 右下角手风琴画廊（作为轮播选择器，严格保持 6 格） ---- */
const galleryRow = document.getElementById('galleryRow');
let panels = [];
galleryBox.style.setProperty('--hold', HERO_HOLD + 'ms');
const HG_RATIO = 0.54;
const GALLERY_MAX_SLOTS = 6;
galleryRow.style.setProperty('--hg-grow', (HG_RATIO / (1 - HG_RATIO) * (GALLERY_MAX_SLOTS - 1)).toFixed(3));

function getGalleryWindow(){
  const items = homeMovies();
  const n = items.length;
  if(n <= GALLERY_MAX_SLOTS){
    return items.map((m, i) => ({movie: m, index: i}));
  }
  const half = Math.floor(GALLERY_MAX_SLOTS / 2);
  let start = current - half;
  if(start < 0) start = 0;
  if(start + GALLERY_MAX_SLOTS > n) start = n - GALLERY_MAX_SLOTS;
  const list = [];
  for(let k = 0; k < GALLERY_MAX_SLOTS; k++){
    const idx = start + k;
    list.push({movie: items[idx], index: idx});
  }
  return list;
}

function renderGallery(){
  galleryRow.replaceChildren();
  panels = [];
  const galleryItems = getGalleryWindow();
  galleryRow.style.setProperty('--hg-grow', (HG_RATIO / (1 - HG_RATIO) * (galleryItems.length - 1)).toFixed(3));
  galleryItems.forEach((item) => {
    const m = item.movie;
    const i = item.index;
    const el = document.createElement('div');
    el.className = 'hg-panel' + (i === current ? ' is-open is-live' : '');
    el.title = m.t;
    el.innerHTML = `
      ${posterMarkup(m)}
      <div class="hg-panel-shade"></div>
      <div class="hg-rating-pill">★ ${m.r ? Number(m.r).toFixed(1) : '—'}</div>
      <div class="hg-collapsed-info">
        <div class="hg-col-title">${m.t.split(/[:：]/)[0]}</div>
        <div class="hg-col-sub">${m.y}</div>
      </div>
      <div class="hg-expanded-info">
        <div class="hg-exp-title">${m.t}</div>
        <div class="hg-exp-sub">${m.y} · ${m.genre || '影视'}</div>
      </div>
      <div class="hg-live-bar"><i></i></div>`;
    el.addEventListener('mouseenter', () => { setOpen(i); heroGoTo(i); });
    el.addEventListener('click', () => openDetail(m, el));
    el.dataset.movieIndex = i;
    galleryRow.appendChild(el);
    el.querySelectorAll('img').forEach(image => bindCardCover(image, {eager: i === current, owner: m}));
    panels.push(el);
  });
  setLive(current);
  setOpen(current);
}
function setLive(i){
  panels.forEach(p => p.classList.toggle('is-live', Number(p.dataset.movieIndex) === i));
}
function setOpen(i){
  panels.forEach(p => p.classList.toggle('is-open', Number(p.dataset.movieIndex) === i));
}
function restartLiveBar(target){
  panels.forEach(p => {
    const bar = p.querySelector('.hg-live-bar i');
    if(bar){
      bar.classList.remove('run');
      if(Number(p.dataset.movieIndex) === target){ void bar.offsetWidth; bar.classList.add('run'); }
    }
  });
}
renderGallery();

/* ---- 轮播调度 ---- */
function scheduleHold(){
  clearTimeout(holdTimer);
  if(morphhold) return;
  holdTimer = setTimeout(() => {
    const ok = currentView === 'home' && !hoverPause && !document.hidden &&
               !document.body.classList.contains('player-active');
    if(ok) heroGoTo(current + 1);
    scheduleHold();
  }, HERO_HOLD);
}
heroEl.addEventListener('mouseenter', () => { hoverPause = true;  galleryBox.classList.add('paused'); });
heroEl.addEventListener('mouseleave', () => { hoverPause = false; galleryBox.classList.remove('paused'); });

function announce(i){
  swapHeroCopy(i);
  const exists = panels.some(p => Number(p.dataset.movieIndex) === i);
  if(!exists){
    renderGallery();
  } else {
    setLive(i);
    setOpen(i);
  }
}
function heroGoTo(target){
  const n = homeMovies().length;
  if(!n) return;
  target = ((target % n) + n) % n;
  if(animating){ pending = target; return; }
  if(target === current) return;
  animating = true;
  domStartTransition(target, () => {
    current = target;
    animating = false;
    const queued = pending;
    pending = null;
    if(queued !== null && queued !== current) heroGoTo(queued);
  }, () => {
    announce(target);
    restartLiveBar(target);
  });
}

/* ---- Hero 轮播：解码优先的双层交叉淡化 + 轻微方向视差 ---- */
const slideCur = document.getElementById('slideCur');
const slideNext = document.getElementById('slideNext');
const imgCur = slideCur.querySelector('img');
const imgNext = slideNext.querySelector('img');
setHeroImage(imgCur, homeMovieAt(current));
setHeroImage(imgNext, homeMovieAt(current));

/* 下一张预加载：轮播停留期间后台拉图，切换零等待 */
const heroPreloadCache = new Map();
function preloadHero(i){
  if(!homeMovies().length) return Promise.resolve();
  const movie = homeMovieAt(i);
  const url = heroPosterUrl(movie);
  // 本地视频探针可能返回很长的 data URI；缓存键只保留稳定的短指纹，
  // 避免 Map 额外长期持有整段 base64 字符串。
  const cacheKey = `${movie?.id ?? movie?.record?.id ?? i}:${url.length}:${url.slice(-64)}`;
  if(heroPreloadCache.has(cacheKey)) return heroPreloadCache.get(cacheKey);
  const im = new Image();
  im.decoding = 'async';
  const ready = new Promise(resolve => {
    const finish = () => resolve();
    im.onload = () => {
      if(typeof im.decode === 'function') im.decode().catch(() => {}).then(finish);
      else finish();
    };
    im.onerror = finish;
  });
  heroPreloadCache.set(cacheKey, ready);
  im.src = url;
  if(im.complete) im.onload();
  while(heroPreloadCache.size > 8) heroPreloadCache.delete(heroPreloadCache.keys().next().value);
  return ready;
}
preloadHero(current + 1);
preloadHero(current - 1);

function prepareHeroImage(image, movie){
  return new Promise(resolve => {
    let settled = false;
    let timeout = null;
    const finish = () => {
      if(settled) return;
      settled = true;
      if(timeout) window.clearTimeout(timeout);
      image.removeEventListener('load', onLoad);
      image.removeEventListener('error', onError);
      resolve();
    };
    const onLoad = () => {
      if(typeof image.decode === 'function') image.decode().catch(() => {}).then(finish);
      else finish();
    };
    const onError = () => {
      // setHeroImage 会先切换到卡片封面或默认图；等待新的 load 事件。
      if(image.getAttribute('src') === '/assets/hero-backdrop.jpg') window.setTimeout(finish, 0);
    };
    image.addEventListener('load', onLoad);
    image.addEventListener('error', onError);
    setHeroImage(image, movie);
    if(image.complete && image.naturalWidth) onLoad();
    timeout = window.setTimeout(() => {
      if(settled) return;
      // 远程/损坏海报卡住时，优先切到现有本地默认图，再继续转场。
      const fallback = '/assets/hero-backdrop.jpg';
      if(image.getAttribute('src') !== fallback){
        image.dataset.heroCardTried = '1';
        image.dataset.heroRemoteTried = '1';
        image.src = fallback;
        if(image.complete && image.naturalWidth) onLoad();
        else window.setTimeout(finish, 120);
      } else {
        finish();
      }
    }, HERO_IMAGE_TIMEOUT_MS);
  });
}

/* 仅动画 opacity/transform，避免全屏 blur 与同步强制重排。 */
async function domStartTransition(target, done, started){
  const items = homeMovies();
  const targetMovie = homeMovieAt(target);
  const forwardDistance = ((target - current) % items.length + items.length) % items.length;
  const direction = forwardDistance && forwardDistance <= items.length / 2 ? 1 : -1;
  heroEl.style.setProperty('--hero-enter-x', (direction * 12) + 'px');
  heroEl.style.setProperty('--hero-exit-x', (direction * -8) + 'px');
  await prepareHeroImage(imgNext, targetMovie);
  heroEl.classList.add('is-hero-resetting');
  heroEl.classList.remove('is-hero-transitioning');
  requestAnimationFrame(() => requestAnimationFrame(() => {
    let finished = false;
    let fallbackTimer = null;
    const finish = () => {
      if(finished) return;
      finished = true;
      slideNext.removeEventListener('transitionend', onEnd);
      if(fallbackTimer) window.clearTimeout(fallbackTimer);
      prepareHeroImage(imgCur, targetMovie).then(() => {
        heroEl.classList.add('is-hero-resetting');
        heroEl.classList.remove('is-hero-transitioning');
        requestAnimationFrame(() => requestAnimationFrame(() => {
          heroEl.classList.remove('is-hero-resetting');
          preloadHero(target + 1);
          preloadHero(target - 1);
          done();
        }));
      });
    };
    const onEnd = (e) => { if(e.target === slideNext && e.propertyName === 'opacity') finish(); };
    slideNext.addEventListener('transitionend', onEnd);
    heroEl.classList.remove('is-hero-resetting');
    heroEl.classList.add('is-hero-transitioning');
    if(started) started();
    fallbackTimer = window.setTimeout(finish, HERO_TRANSITION_MS + 100);
  }));
}

/* morphhold：冻结在转场中途，便于截图预览淡入效果 */
if(morphhold && homeMovies().length){
  setHeroImage(imgNext, homeMovieAt(current + 1));
  heroEl.classList.add('is-hero-transitioning');
  slideNext.style.opacity = '.48';
} else {
  restartLiveBar(current);
  scheduleHold();
}

/* ================= 媒体库网格 ================= */
const libGrid = document.getElementById('libGrid');
const STAR_SVG = '<svg viewBox="0 0 24 24"><path d="M12 2.8l2.8 5.7 6.3.9-4.6 4.4 1.1 6.3-5.6-3-5.6 3 1.1-6.3L2.9 9.4l6.3-.9z"/></svg>';
let activeFilter = 'all';
let activeTypeFilter = 'all';
let libraryCategoryRenderSignature = '';
let activeYear = '年份';
let searchTerm = '';
let sortMode = 'rating';
let activeSourceFilter = 'all';
function librarySourceKey(media){
  if(appMode === 'catalog') return 'TVMaze 公共目录';
  const raw = String(media?.sourceLabel || media?.sourceType || media?.network || '已连接来源').trim();
  return raw || '已连接来源';
}
function backendSourceKeyForLibrarySource(sourceKey){
  // 来源面板的键是显示名（librarySourceKey 优先取 sourceLabel），
  // 这里必须映射回库内真实的 source_type，否则 DELETE 匹配不到行。
  const names = {
    'provider:guangya': 'provider:guangya',
    'provider:streamhub': 'provider:streamhub',
    'local': 'local',
    'provider:local': 'local',
    '本地文件': 'local',
    'OpenList 云盘': 'openlist',
    '光鸭云盘': 'provider:guangya',
    'StreamHub 本机媒体中心': 'provider:streamhub'
  };
  return names[sourceKey] || sourceKey;
}
function librarySourceName(key){
  const names = {
    'provider:guangya': '光鸭云盘',
    'provider:streamhub': 'StreamHub 媒体中心',
    'local': '本地磁盘',
    'provider:local': '本地磁盘',
    '本地文件': '本地磁盘'
  };
  return names[key] || key;
}
function librarySourceIcon(key){
  if(/guangya|光鸭/i.test(key)) return '云';
  if(/streamhub/i.test(key)) return 'SH';
  if(/local|本地/i.test(key)) return '▰';
  if(/tvmaze/i.test(key)) return 'TV';
  return '源';
}
function librarySourceItems(includeAdult = false){
  const groups = new Map();
  MOVIES.forEach(media => {
    if(!includeAdult && media.adult) return;
    const key = librarySourceKey(media);
    if(!groups.has(key)) groups.set(key, []);
    groups.get(key).push(media);
  });
  return [...groups.entries()].map(([key, items]) => ({key, name:librarySourceName(key), items}));
}
function librarySourceMedia(sourceKey){
  return MOVIES.filter(media => librarySourceKey(media) === sourceKey);
}
function mediaTypeKey(movie){
  if(!movie) return 'video';
  const raw = String(movie.type || movie.metadata?.mediaType || movie.q || '').toLowerCase();
  if(movie.type === 'series' || /^(tv|show|episode|series)$/.test(raw) || movie.q === '电视剧' || movie.q === 'TV SERIES' || (Array.isArray(movie.episodes) && movie.episodes.length > 1)) return 'series';
  if(/movie|film|电影/.test(raw)) return 'movie';
  return 'video';
}
function mediaTypeLabel(key){ return ({all:'全部',series:'电视剧',movie:'电影',video:'未识别类型'})[key] || key; }
function renderLibraryCategories(){
  const root = document.getElementById('libraryCategoryLevel');
  if(!root) return;
  const typeCounts = new Map(), genreCounts = new Map();
  MOVIES.forEach(movie => {
    // 与 renderGrid 的 18+ 隔离保持一致：成人条目不参与类型/题材统计，
    // 否则分类栏会露出 18+ 影片的题材标签（点进去却没有对应卡片）。
    if(movie.adult) return;
    const type = mediaTypeKey(movie); typeCounts.set(type, (typeCounts.get(type) || 0) + 1);
    const genres = Array.isArray(movie.genres) && movie.genres.length ? movie.genres : [movie.genre || '未分类'];
    genres.filter(genre => genre && !isAdultGenreLabel(genre)).forEach(genre => genreCounts.set(String(genre), (genreCounts.get(String(genre)) || 0) + 1));
  });
  if(activeTypeFilter !== 'all' && !typeCounts.has(activeTypeFilter)) activeTypeFilter = 'all';
  if(activeFilter !== 'all' && !genreCounts.has(activeFilter)) activeFilter = 'all';
  const signature = `${[...typeCounts.entries()].join('|')}::${[...genreCounts.entries()].join('|')}::${activeTypeFilter}::${activeFilter}`;
  if(signature === libraryCategoryRenderSignature && root.childElementCount) return;
  libraryCategoryRenderSignature = signature;
  const makeButton = (key, label, count, kind) => `<button type="button" class="pill ${(kind === 'type' ? activeTypeFilter : activeFilter) === key ? 'active' : ''}" data-category-kind="${kind}" data-category-value="${escapeHtml(key)}">${escapeHtml(label)} <small>${count}</small></button>`;
  const catalogCount = [...typeCounts.values()].reduce((sum, value) => sum + value, 0);
  const types = [['all', catalogCount], ...[...typeCounts.entries()].sort((a,b) => a[0].localeCompare(b[0])).map(([key,count]) => [key,count])];
  const genres = [['all', catalogCount], ...[...genreCounts.entries()].sort((a,b) => b[1] - a[1] || a[0].localeCompare(b[0], 'zh-CN'))];
  root.innerHTML = `<div class="library-category-row"><span class="library-category-label">类型</span>${types.map(([key,count]) => makeButton(key, mediaTypeLabel(key), count, 'type')).join('')}</div><div class="library-category-row"><span class="library-category-label">题材</span>${genres.map(([key,count]) => makeButton(key, key === 'all' ? '全部题材' : key, count, 'genre')).join('')}</div>`;
  root.querySelectorAll('[data-category-kind]').forEach(item => item.addEventListener('click', () => {
    if(item.dataset.categoryKind === 'type') activeTypeFilter = item.dataset.categoryValue || 'all';
    else activeFilter = item.dataset.categoryValue || 'all';
    renderLibraryCategories(); renderGrid();
  }));
}
function renderLibrarySources(){
  const manager = document.getElementById('librarySourceManager');
  const list = document.getElementById('librarySourceList');
  const count = document.getElementById('librarySourceManagerCount');
  if(!list) return;
  const items = librarySourceItems();
  const signature = items.map(item => `${item.key}:${item.items.length}`).join('|') + `|${activeSourceFilter}|${isNativeMediaMode()}`;
  if(signature === librarySourceRenderSignature && list.childElementCount) return;
  librarySourceRenderSignature = signature;
  if(manager) manager.hidden = !isNativeMediaMode() || !items.length;
  if(count) count.textContent = String(items.length);
  if(!items.length){
    list.innerHTML = '<div class="library-source-empty">还没有可管理的媒体来源。</div>';
    return;
  }
  const native = isNativeMediaMode();
  list.innerHTML = items.map(item => {
    const managed = native && item.key !== 'TVMaze 公共目录';
    return `<div class="library-source-row ${activeSourceFilter === item.key ? 'active' : ''}" data-source-key="${escapeHtml(item.key)}">
      <span class="library-source-icon">${escapeHtml(librarySourceIcon(item.key))}</span>
      <button type="button" class="library-source-main" data-source-filter="${escapeHtml(item.key)}"><span class="library-source-name">${escapeHtml(item.name)}</span><small class="library-source-meta">${item.items.length} 条媒体</small></button>
      ${managed ? `<span class="library-source-actions"><button type="button" class="library-source-action" data-source-move="${escapeHtml(item.key)}" title="移动此来源中的媒体" aria-label="移动此来源中的媒体">↔</button><button type="button" class="library-source-action delete" data-source-delete="${escapeHtml(item.key)}" title="从影视库移除此来源" aria-label="从影视库移除此来源">×</button></span>` : '<span></span>'}
    </div>`;
  }).join('');
  list.querySelectorAll('[data-source-filter]').forEach(button => button.addEventListener('click', () => {
    activeSourceFilter = button.dataset.sourceFilter || 'all';
    renderLibrarySources();
    renderGrid();
  }));
  list.querySelectorAll('[data-source-move]').forEach(button => button.addEventListener('click', event => {
    event.stopPropagation();
    moveSourceToLibrary(button.dataset.sourceMove);
  }));
  list.querySelectorAll('[data-source-delete]').forEach(button => button.addEventListener('click', event => {
    event.stopPropagation();
    removeSourceFromLibrary(button.dataset.sourceDelete);
  }));
}
let gridRenderGeneration = 0;
let lastGridQuerySignature = '';
function renderGrid(){
  const querySignature = `${activeTypeFilter}|${activeFilter}|${activeYear}|${searchTerm}|${sortMode}|${activeSourceFilter}`;
  lastGridQuerySignature = querySignature;
  const generation = ++gridRenderGeneration;
  if(window.__ttvLibraryGridObserver){
    window.__ttvLibraryGridObserver.disconnect();
    window.__ttvLibraryGridObserver = null;
  }
  renderLibrarySources();
  renderLibraryCategories();
  libGrid.innerHTML = '';
  let visible = MOVIES.filter(m => {
    // 18+ 内容完全隔离：常规影视库、搜索与分类筛选一律不显示
    if(m.adult) return false;
    const matchesType = activeTypeFilter === 'all' || mediaTypeKey(m) === activeTypeFilter;
    const matchesFilter = activeFilter === 'all' || (m.genres && m.genres.includes(activeFilter)) || m.genre === activeFilter;
    const matchesYear = activeYear === '年份' || String(m.y) === activeYear;
    const haystack = [
      m.t, m.sourceTitle, m.seriesTitle, m.summary, m.genre,
      Array.isArray(m.genres) ? m.genres.join(' ') : '',
      m.record?.originalTitle, m.record?.remotePath,
      m.record?.payload?.folderPath, m.record?.payload?.folderName
    ].join(' ').toLowerCase();
    const matchesSearch = !searchTerm || haystack.includes(searchTerm);
    const matchesSource = activeSourceFilter === 'all' || librarySourceKey(m) === activeSourceFilter;
    return matchesType && matchesFilter && matchesYear && matchesSearch && matchesSource;
  });

  if(sortMode === 'rating'){
    visible.sort((a, b) => (b.r || 0) - (a.r || 0));
  } else if(sortMode === 'year'){
    visible.sort((a, b) => (parseInt(b.y) || 0) - (parseInt(a.y) || 0));
  } else if(sortMode === 'title'){
    visible.sort((a, b) => a.t.localeCompare(b.t, 'zh-CN'));
  }

  const resultCountEl = document.getElementById('libraryResultCount');
  if(resultCountEl) resultCountEl.textContent = `共 ${visible.length} 条${appMode === 'streamhub' ? ' StreamHub 媒体' : (appMode === 'desktop' ? '本地媒体' : '公开目录内容')}`;

  if(!visible.length){
    libGrid.innerHTML = '<p class="catalog-empty">没有找到匹配的影视内容，请尝试其他关键词或筛选条件。</p>';
    return;
  }
  // Infinite scroll: append a small batch only when the sentinel approaches
  // the viewport. This keeps the initial layout light and avoids a click that
  // synchronously rebuilds the entire grid.
  const batchSize = 40;
  let rendered = 0;
  let batchPending = false;
  const sentinel = document.createElement('div');
  sentinel.className = 'library-grid-sentinel';
  sentinel.setAttribute('aria-hidden', 'true');
  const appendBatch = () => {
    if(batchPending || generation !== gridRenderGeneration) return;
    batchPending = true;
    const schedule = window.requestAnimationFrame || ((callback) => window.setTimeout(callback, 16));
    schedule(() => {
      batchPending = false;
      if(generation !== gridRenderGeneration) return;
    const fragment = document.createDocumentFragment();
    const end = Math.min(rendered + batchSize, visible.length);
    for(let i = rendered; i < end; i++){
      const m = visible[i];
      const el = document.createElement('div');
      el.className = 'movie-card' + (i < 24 ? ' rise-in' : '');
      el.dataset.movieId = String(m.id ?? '');
      if(i < 24) el.style.animationDelay = (i * 35) + 'ms';
    const stars = STAR_SVG.repeat(5);
    const ratingLabel = m.r > 0 ? m.r.toFixed(1) : '—';
    const libraryActions = '';
    el.innerHTML = `
      <div class="fc-scene">
        <div class="fc-inner">
          <div class="fc-face fc-front">
            ${posterMarkup(m)}
            <span class="badge q-badge">${m.q}</span>${m.adult ? '<span class="badge adult-badge">18+</span>' : ''}
          </div>
          <div class="fc-face fc-back">
            <div class="fc-back-bg"><img data-cover-src="${m.img}" alt="" width="400" height="600" loading="lazy" decoding="async"/></div>
            <div class="fc-body">
              <div class="fc-title">${m.t}</div>
              <div class="fc-rate">
                <div class="fc-score">${ratingLabel}</div>
                <div>
                  <div class="fc-stars">
                    <div class="fc-stars-row fc-stars-base">${stars}</div>
                    <div class="fc-stars-row fc-stars-fill" style="width:${m.r * 10}%">${stars}</div>
                  </div>
              <div class="fc-votes">${appMode === 'catalog' ? 'TVMaze' : (m.sourceLabel || '本地媒体')} · ${m.v}${m.libraryId ? ' · ' + escapeHtml(m.libraryId) : ''}</div>
                </div>
              </div>
              <div class="fc-chips">
                <span>${m.y}</span><span>${m.d}</span><span>${m.genre}</span><span>${m.q}</span>${m.adult ? '<span class="adult-chip">18+</span>' : ''}
              </div>
              <p class="fc-desc">${escapeHtml(m.summary)}</p>
              <div class="fc-actions">
                <button class="fc-play" data-act="play"><svg viewBox="0 0 24 24" fill="#fff"><path d="M8 5.5v13l11-6.5z"/></svg>立即播放</button>
                <button class="fc-fav" data-act="fav" title="收藏" aria-pressed="${favoriteIds.has(String(m.id))}"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 5v14M5 12h14"/></svg></button>
                ${libraryActions}
              </div>
            </div>
          </div>
        </div>
      </div>
      <div class="m-meta">
        <div class="m-title">${m.t}</div>
        <div class="m-sub"><b>★ ${ratingLabel}</b> ${m.y} · ${m.d}</div>
      </div>`;
    bindFlipCardCovers(el, m, {eagerFront: i < COVER_EAGER_COUNT && rendered === 0});
    el.addEventListener('click', () => openDetail(m, el));
    el.querySelector('[data-act="play"]').addEventListener('click', e => { e.stopPropagation(); openPlayer(m, el); });
    el.querySelector('[data-act="fav"]').addEventListener('click', e => { e.stopPropagation(); toggleFavorite(m); });
      fragment.appendChild(el);
    }
    libGrid.appendChild(fragment);
    rendered = end;
    if(rendered < visible.length && generation === gridRenderGeneration){
      libGrid.appendChild(sentinel);
      if(typeof IntersectionObserver !== 'function'){
        // Older embedded webviews may not expose IntersectionObserver; keep
        // the same non-blocking behavior with a frame-scheduled fallback.
        if(typeof window.requestIdleCallback === 'function'){
          window.requestIdleCallback(() => appendBatch(), {timeout: 200});
        }else{
          window.setTimeout(() => appendBatch(), 32);
        }
      }else if(!window.__ttvLibraryGridObserver){
        window.__ttvLibraryGridObserver = new IntersectionObserver(entries => {
          if(entries.some(entry => entry.isIntersecting)) appendBatch();
        }, {root: null, rootMargin: '720px 0px', threshold: 0});
      }
      window.__ttvLibraryGridObserver.observe(sentinel);
    }else if(window.__ttvLibraryGridObserver){
      window.__ttvLibraryGridObserver.disconnect();
      window.__ttvLibraryGridObserver = null;
    }
    });
  };
  appendBatch();
}
renderGrid();

/* ================= 短剧放映厅 ================= */
const SHORT_DRAMA_SEED = [
  {id:'7673056752958458942',title:'好雨知时节',coverUrl:'https://p3-novel.byteimg.com/novel-pic/05438bf540034a849cd2dbd3a28db8cd~tplv-shrink:640:0.image',episodes:'全89集',category:'爱情 / 都市爱情 / 日久生情',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7673056752958458942',description:'好雨知时节 · 爱情 · 都市爱情 · 日久生情'},
  {id:'7673178645438925848',title:'老公的工资，婆婆的账',coverUrl:'https://p6-novel.byteimg.com/novel-pic/17e9bfb9681e4c86c260fca3689e6125~tplv-shrink:640:0.image',episodes:'全65集',category:'家庭 / 家庭伦理 / 真相大白',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7673178645438925848',description:'家庭关系与生活真相交织的短剧。'},
  {id:'7673415977467382809',title:'闪婚，她甜的过头',coverUrl:'https://p3-novel.byteimg.com/novel-pic/e71010b91c85455652074372dfe0fb75~tplv-shrink:640:0.image',episodes:'全78集',category:'爱情 / 都市爱情 / 先婚后爱',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7673415977467382809',description:'都市爱情 · 先婚后爱。'},
  {id:'7677607948964596798',title:'今天裴总和月亮谈恋爱了吗2',coverUrl:'https://p6-novel.byteimg.com/novel-pic/dd6ba4a47e21fbec38cc7fe2f2fb0209~tplv-shrink:640:0.image',episodes:'全70集',category:'爱情 / 都市爱情 / 先婚后爱',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7677607948964596798',description:'都市情感系列短剧第二部。'},
  {id:'7669732448577522713',title:'猛龙过江，绝不回头',coverUrl:'https://p6-novel.byteimg.com/novel-pic/db669cc9ac6d39e4b7e1ab49f1ddfae8~tplv-shrink:640:0.image',episodes:'全81集',category:'都市 / 逆袭翻身 / 打脸反派',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7669732448577522713',description:'都市逆袭爽剧。'},
  {id:'7676353732207971390',title:'休个假，顺便成了老板娘',coverUrl:'https://p3-novel.byteimg.com/novel-pic/fcbaf8dc7c7667f8355d3ccfa25ae08c~tplv-shrink:640:0.image',episodes:'全77集',category:'爱情 / 都市爱情 / 日久生情',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7676353732207971390',description:'轻松都市爱情短剧。'},
  {id:'7672388285074787352',title:'我的女儿你要向前走',coverUrl:'https://p3-novel.byteimg.com/novel-pic/3bf2b77dd7478d6e04b6ed14b2082ade~tplv-shrink:640:0.image',episodes:'全72集',category:'家庭 / 家庭伦理 / 矛盾和解',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7672388285074787352',description:'围绕亲情与成长展开的家庭短剧。'},
  {id:'7673359550325459992',title:'当家主母的自我修养：从掌掴装货开始',coverUrl:'https://p3-novel.byteimg.com/novel-pic/6ee29aa16056cf1421b2ee3ae7e5498a~tplv-shrink:640:0.image',episodes:'全83集',category:'成长 / 女性成长 / 穿越',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7673359550325459992',description:'女性成长与穿越题材短剧。'},
  {id:'7671125827500657688',title:'全能后妈，爆改全家',coverUrl:'https://p6-novel.byteimg.com/novel-pic/472c9fde88c4019272a5326a3bb6ea79~tplv-shrink:640:0.image',episodes:'全71集',category:'家庭 / 家庭伦理 / 亲情治愈',source:'红果短剧官网',sourceUrl:'https://hongguoduanju.com/detail?series_id=7671125827500657688',description:'家庭伦理与亲情治愈短剧。'}
];
const COMIC_DRAMA_SEED = [
  {id:'7677801492920667198',title:'糯糯下山，师兄们都慌了',coverUrl:'https://p6-novel.byteimg.com/novel-pic/596f73a90e0c4cff2d95ee3b1afd4268~tplv-shrink:640:0.image',episodes:'漫剧',category:'萌宝 / 异界',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7677801492920667198',description:'六岁那年，云糯糯被师父派下山历练，顺便寻找失散多年的五位师兄。'},
  {id:'7677917481222032408',title:'精神小妹也要修机甲吗第四季',coverUrl:'https://p3-novel.byteimg.com/novel-pic/6a54fd78e689379aae168dcc636efc2d~tplv-shrink:640:0.image',episodes:'漫剧',category:'科幻 / 穿越 / 异界',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7677917481222032408',description:'科幻穿越向动态漫剧。'},
  {id:'7677914379060251673',title:'聚宝仙盆之杂灵根才是真BOSS第十季',coverUrl:'https://p6-novel.byteimg.com/novel-pic/8ffad6c82f7fc13b782ce00a82abcd5c~tplv-shrink:640:0.image',episodes:'漫剧',category:'玄幻 / 修真 / 逆袭',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7677914379060251673',description:'玄幻修真逆袭漫剧。'},
  {id:'7673067074104593432',title:'谁说纨绔不能当状元',coverUrl:'https://p6-novel.byteimg.com/novel-pic/11cc0d4546645a32992f76061c0bc3a7~tplv-shrink:640:0.image',episodes:'漫剧',category:'脑洞 / 穿越 / 逆袭',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7673067074104593432',description:'穿越逆袭题材漫剧。'},
  {id:'7678338044344159256',title:'满院亲戚全是上古大妖第二季',coverUrl:'https://p3-novel.byteimg.com/novel-pic/5e8df9414e8cda8638aba045b17f99a9~tplv-shrink:640:0.image',episodes:'漫剧',category:'奇幻 / 年代',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7678338044344159256',description:'奇幻年代向动态漫剧。'},
  {id:'7675288927619533886',title:'咱家剑宗团宠小师妹第五季',coverUrl:'https://p3-novel.byteimg.com/novel-pic/40c6fe1101466d93cbf562c890e16dc0~tplv-shrink:640:0.image',episodes:'漫剧',category:'萌宝 / 修真 / 异界',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7675288927619533886',description:'剑宗团宠向动态漫剧。'},
  {id:'7675678593866812440',title:'大将军扛楼养活百万大军',coverUrl:'https://p3-novel.byteimg.com/novel-pic/142b77b35fd88e74dc9ebfb8658e6226~tplv-shrink:640:0.image',episodes:'漫剧',category:'脑洞 / 穿越 / 都市',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7675678593866812440',description:'脑洞穿越都市漫剧。'},
  {id:'7677874167550594110',title:'谁说纨绔不能当状元第二季',coverUrl:'https://p6-novel.byteimg.com/novel-pic/29e3546d2552a1b71bb03212c037db65~tplv-shrink:640:0.image',episodes:'漫剧',category:'剧情 / 穿越 / 古代',kind:'comic',source:'红果漫剧',sourceUrl:'https://hongguoduanju.com/detail?series_id=7677874167550594110',description:'古代穿越剧情漫剧。'}
];
/* ============ 短剧放映厅（红果公开目录 · 无限卡片流 · 站内播放） ============ */
// 官网目录树快照（selectorList），服务端 facet 过滤直接映射官网查询串。
const SHORT_DRAMA_CATEGORIES = [
  {name:'现代', facet:'background=cate_757'},
  {name:'都市', facet:'background=cate_1'},
  {name:'古代', facet:'background=cate_758'},
  {name:'乡村', facet:'background=cate_11'},
  {name:'年代', facet:'background=cate_79'},
  {name:'架空', facet:'background=cate_452'},
  {name:'职场', facet:'background=cate_127'},
  {name:'民国', facet:'background=cate_390'},
  {name:'校园', facet:'background=cate_4'},
  {name:'宫廷', facet:'background=cate_1153'},
  {name:'现言', facet:'topic=cate_1021'},
  {name:'古言', facet:'topic=cate_439'},
  {name:'女性成长', facet:'topic=cate_1048'},
  {name:'脑洞', facet:'topic=cate_262'},
  {name:'奇幻', facet:'topic=cate_1020'},
  {name:'玄幻', facet:'topic=cate_1019'},
  {name:'战神', facet:'topic=cate_1038'},
  {name:'宫斗', facet:'topic=cate_246'},
  {name:'仙侠', facet:'topic=cate_1013'},
  {name:'权谋', facet:'topic=cate_1047'},
  {name:'悬疑', facet:'topic=cate_165'},
  {name:'喜剧', facet:'topic=cate_303'},
  {name:'青春', facet:'topic=cate_297'},
  {name:'科幻', facet:'topic=cate_1092'},
  {name:'刑侦', facet:'topic=cate_1148'},
  {name:'抗战', facet:'topic=cate_504'},
  {name:'武侠', facet:'topic=cate_1172'},
  {name:'大女主', facet:'setting=cate_760'},
  {name:'大男主', facet:'setting=cate_1207'},
  {name:'重生', facet:'setting=cate_36'},
  {name:'穿越', facet:'setting=cate_37'},
  {name:'马甲', facet:'setting=cate_266'},
  {name:'系统', facet:'setting=cate_19'},
  {name:'先婚后爱', facet:'setting=cate_265'},
  {name:'破镜重圆', facet:'setting=cate_475'},
  {name:'赘婿逆袭', facet:'setting=cate_1044'},
  {name:'打脸虐渣', facet:'setting=cate_1051'},
  {name:'神豪', facet:'setting=cate_20'},
  {name:'豪门', facet:'setting=cate_936'},
  {name:'甜宠', facet:'setting=cate_96'},
  {name:'追妻火葬场', facet:'setting=cate_616'},
  {name:'萌宠', facet:'setting=cate_428'}
];
const COMIC_DRAMA_CATEGORIES = [
  {name:'萌宝', facet:'萌宝'},
  {name:'异界', facet:'异界'},
  {name:'玄幻', facet:'玄幻'},
  {name:'修真', facet:'修真'},
  {name:'穿越', facet:'穿越'},
  {name:'科幻', facet:'科幻'},
  {name:'逆袭', facet:'逆袭'},
  {name:'都市', facet:'都市'},
  {name:'古代', facet:'古代'},
  {name:'奇幻', facet:'奇幻'},
  {name:'脑洞', facet:'脑洞'},
  {name:'年代', facet:'年代'},
  {name:'民国', facet:'民国'},
  {name:'系统', facet:'系统'},
  {name:'甜宠', facet:'甜宠'},
  {name:'战神', facet:'战神'}
];
const shortDramaState = {
  channel: 'short',
  items: [], seen: new Set(), cursor: null,
  itemById: new Map(),
  facet: '', query: '', gender: '', time: '', sort: '1',
  loading: false, started: false, allDupStreak: 0, chain: 0, requestId: 0,
  detailCache: new Map(), detailInflight: new Map(), detailOpenRequestId: 0,
  webPlaybackCache: new Map(), webPlaybackInflight: new Map(), streamPlaybackCache: new Map(), streamInflight: new Map(),
  resolvedPlaybackCache: new Map(), resolvedPlaybackInflight: new Map(),
  appStatus: null, appStatusPromise: null, playRequestId: 0,
  channelCache: {short:null, comic:null},
  catalogPrefetch: {short:null, comic:null},
  nextPage: {short:null, comic:null}
};
function isComicDramaChannel(){
  return shortDramaState.channel === 'comic';
}
function isHongguoPlaybackId(id){
  const value = String(id || '');
  return value.startsWith('shortdrama:') || value.startsWith('comicdrama:');
}
window.isHongguoPlaybackId = isHongguoPlaybackId;
function hongguoKindOf(item){
  if(item?.kind === 'comic' || item?.channel === 'comic') return 'comic';
  if(String(item?.source || '').includes('漫剧')) return 'comic';
  return isComicDramaChannel() ? 'comic' : 'short';
}
function hongguoAppProfile(detail){
  const comic = detail?.kind === 'comic' || hongguoKindOf(detail) === 'comic';
  return comic
    ? {contentType:1004, appId:8704}
    : {contentType:1, appId:8662};
}
let shortDramaSearchTimer = null;
let shortDramaObserver = null;
let shortDramaCtx = null;
let shortDramaAdvancing = false;
let shortDramaAutoAdvanceAt = 0;
let shortDramaAutoAdvanceKey = '';
let shortDramaNativeRecoveryKey = '';
let shortDramaNextTimer = null;
let shortDramaNextDeadline = 0;
let shortDramaNextFromTail = false;
let shortDramaPreparedNext = null;
let shortDramaBufferEl = null;
function shortDramaCardMarkup(item){
  const comic = hongguoKindOf(item) === 'comic';
  const label = comic ? '红果漫剧' : '红果短剧';
  const fallbackKind = comic ? '漫剧' : '短剧';
  const title = escapeHtml(item.title || `未命名${fallbackKind}`);
  const cover = escapeHtml(item.coverUrl || '/assets/detail-poster.jpg');
  const category = escapeHtml(item.category || fallbackKind);
  const episodes = escapeHtml(item.episodes || fallbackKind);
  const source = escapeHtml(item.source || label);
  const description = escapeHtml(item.description || (comic ? '来自红果漫剧热播榜的条目。' : '来自公开短剧目录的条目。'));
  const chips = String(item.category || fallbackKind).split(/[\/／]/).map(value => value.trim()).filter(Boolean).slice(0, 4);
  return `<article class="movie-card short-drama-card" data-short-drama-id="${escapeHtml(item.id || '')}" data-short-drama-text="${escapeHtml(`${item.title || ''} ${item.category || ''} ${item.description || ''}`.toLowerCase())}">
    <div class="fc-scene">
      <div class="fc-inner">
        <div class="fc-face fc-front">
          <img class="card-cover is-pending" data-cover-src="${cover}" alt="${title}" width="400" height="600" loading="lazy" decoding="async">
          <span class="badge short-drama-card-badge">${label}</span>
          <span class="badge q-badge">${episodes}</span>
        </div>
        <div class="fc-face fc-back">
          <div class="fc-back-bg"><img data-cover-src="${cover}" alt="" width="400" height="600" loading="lazy" decoding="async"></div>
          <div class="fc-body">
            <div class="fc-title">${title}</div>
            <div class="fc-rate">
              <div class="fc-votes">${source} · ${episodes}</div>
            </div>
            <div class="fc-chips">${chips.map(chip => `<span>${escapeHtml(chip)}</span>`).join('')}</div>
            <p class="fc-desc">${description}</p>
            <div class="fc-actions"><button class="fc-play" type="button" data-act="play"><svg viewBox="0 0 24 24" fill="#fff"><path d="M8 5.5v13l11-6.5z"/></svg>播放首集</button></div>
          </div>
        </div>
      </div>
    </div>
    <div class="m-meta">
      <div class="m-title">${title}</div>
      <div class="m-sub"><b>${label}</b> ${category} · ${episodes}</div>
    </div>
  </article>`;
}
function appendShortDramaCards(items){
  const grid = document.getElementById('shortDramaGrid');
  if(!grid) return;
  const stale = grid.querySelector('.short-drama-loading, .short-drama-empty');
  if(stale) stale.remove();
  // 漫剧“最新”排序在插入前完成，避免追加后对全部 DOM 卡片重排。
  if(isComicDramaChannel() && shortDramaState.sort === '2'){
    items = items.slice().sort((a, b) => comicDramaSeriesTime(b, null) - comicDramaSeriesTime(a, null));
  }
  const fragment = document.createDocumentFragment();
  const startRank = grid.querySelectorAll('.short-drama-card').length;
  items.forEach((item, index) => {
    const template = document.createElement('template');
    template.innerHTML = shortDramaCardMarkup(item).trim();
    const card = template.content.firstElementChild;
    if(!card) return;
    const id = String(item?.id || card.dataset.shortDramaId || '');
    card.dataset.sdRank = String(startRank + index);
    if(item) shortDramaState.itemById.set(id, item);
    // 过滤用元数据只在这里算一次，后续筛选全部读 dataset，避免全量重扫。
    card.dataset.sdGender = comicDramaGenderOf(item, card);
    card.dataset.sdTime = String(comicDramaSeriesTime(item, card));
    card.dataset.sdHidden = '0';
    card.style.setProperty('--sd-enter-delay', `${Math.min(index, 11) * 28}ms`);
    bindFlipCardCovers(card, item, {eagerFront: startRank === 0 && index < COVER_EAGER_COUNT});
    card.addEventListener('click', () => openShortDramaDetail(item, card));
    const prefetchDetail = () => prefetchShortDramaDetail(item);
    card.addEventListener('pointerenter', prefetchDetail, {once:true});
    card.addEventListener('focusin', prefetchDetail, {once:true});
    card.querySelector('[data-act="play"]')?.addEventListener('click', event => {
      event.stopPropagation();
      openShortDramaDetail(item, card, {autoplay:true});
    });
    fragment.appendChild(card);
  });
  grid.appendChild(fragment);
  applyShortDramaFilter();
  scheduleShortDramaDetailPrefetch(items, startRank === 0 ? 8 : 4);
}
function renderShortDramaCategoryChips(){
  const container = document.getElementById('shortDramaCategories');
  if(!container) return;
  const chips = isComicDramaChannel()
    ? [{name:'全部', facet:''}, ...COMIC_DRAMA_CATEGORIES]
    : [{name:'全部', facet:''}, ...SHORT_DRAMA_CATEGORIES];
  container.innerHTML = chips.map(chip =>
    `<button class="pill${chip.facet === shortDramaState.facet ? ' active' : ''}" type="button" data-short-facet="${escapeHtml(chip.facet)}">${escapeHtml(chip.name)}</button>`
  ).join('');
}
function shortDramaFacetQuery(){
  const parts = [];
  if(shortDramaState.facet) parts.push(shortDramaState.facet);
  if(shortDramaState.gender !== '') parts.push(`gender=${shortDramaState.gender}`);
  if(shortDramaState.time !== '') parts.push(`time=${shortDramaState.time}`);
  parts.push(`sort_type=${shortDramaState.sort || '1'}`);
  return parts.join('&');
}
function comicDramaGenderOf(item, card){
  const text = `${item?.category || ''} ${item?.description || ''} ${item?.title || ''} ${card?.dataset?.shortDramaText || ''}`;
  const femaleHints = /女频|女主|大女主|甜宠|追妻|先婚后爱|破镜重圆|女性成长|现言|古言|萌宝|宫斗|权谋|宠妃|嫡女/;
  const maleHints = /男频|男主|大男主|战神|赘婿|神豪|打脸|逆袭|修真|玄幻|机甲|抗战|刑侦|系统/;
  const female = femaleHints.test(text);
  const male = maleHints.test(text);
  if(female === male) return '';
  return female ? '0' : '1';
}
function comicDramaSeriesTime(item, card){
  const id = String(item?.id || card?.dataset?.shortDramaId || '');
  if(!/^\d{15,}$/.test(id)) return 0;
  try{ return Number(BigInt(id) >> 32n) * 1000; }
  catch{ return 0; }
}
function comicDramaTimeWindowMs(value){
  if(value === '1') return 7 * 86400000;
  if(value === '2') return 14 * 86400000;
  if(value === '3') return 30 * 86400000;
  if(value === '4') return 90 * 86400000;
  return 0;
}
function reorderComicDramaCards(){
  // 仅在用户切换漫剧排序时对现有卡片做一次重排；常规追加不再全量移动 DOM。
  if(!isComicDramaChannel()) return;
  const grid = document.getElementById('shortDramaGrid');
  if(!grid) return;
  const newestFirst = shortDramaState.sort === '2';
  const cards = [...grid.querySelectorAll('.short-drama-card')];
  cards.sort((a, b) => {
    if(newestFirst){
      const delta = (Number(b.dataset.sdTime) || 0) - (Number(a.dataset.sdTime) || 0);
      if(delta) return delta;
    }
    return (Number(a.dataset.sdRank) || 0) - (Number(b.dataset.sdRank) || 0);
  });
  const fragment = document.createDocumentFragment();
  cards.forEach(card => fragment.appendChild(card));
  grid.appendChild(fragment);
}
function applyShortDramaFilter(){
  const query = shortDramaState.query.trim().toLowerCase();
  const comic = isComicDramaChannel();
  const localFacet = comic ? shortDramaState.facet.trim().toLowerCase() : '';
  const gender = comic ? shortDramaState.gender : '';
  const timeWindow = comic ? comicDramaTimeWindowMs(shortDramaState.time) : 0;
  const grid = document.getElementById('shortDramaGrid');
  const cards = grid ? [...grid.querySelectorAll('.short-drama-card')] : [];
  // “最新”窗口基准：取已加载卡片里的最大时间戳（榜单时间通常贴近当下）。
  let latestSeriesTime = 0;
  cards.forEach(card => {
    const time = Number(card.dataset.sdTime) || 0;
    if(time > latestSeriesTime) latestSeriesTime = time;
  });
  const now = (latestSeriesTime && Math.abs(Date.now() - latestSeriesTime) > 365 * 86400000)
    ? latestSeriesTime
    : Date.now();
  let visible = 0;
  const filterActive = Boolean(query || localFacet || gender || timeWindow);
  cards.forEach(card => {
    let match;
    if(!filterActive){
      match = true;
    }else{
      const text = card.dataset.shortDramaText || '';
      const genderValue = card.dataset.sdGender || '';
      const seriesTime = Number(card.dataset.sdTime) || 0;
      match = (!query || text.includes(query))
        && (!localFacet || text.includes(localFacet))
        && (!gender || !genderValue || genderValue === gender)
        && (!timeWindow || (seriesTime > 0 && now - seriesTime <= timeWindow));
    }
    const hidden = match ? '0' : '1';
    if(card.dataset.sdHidden !== hidden){
      // 只在状态变化时触碰 class，避免每页追加都触发几百次样式失效。
      card.dataset.sdHidden = hidden;
      card.classList.toggle('sd-filtered-out', !match);
    }
    if(match) visible++;
  });
  const result = document.getElementById('shortDramaResultCount');
  const kind = comic ? '漫剧' : '短剧';
  const filtered = Boolean(query || localFacet || gender || timeWindow || (comic && shortDramaState.sort === '2'));
  if(result){
    result.textContent = shortDramaState.loading && !shortDramaState.items.length
      ? `正在加载${kind}…`
      : filtered
        ? (visible ? `已加载 ${shortDramaState.items.length} 部，匹配 ${visible} 部` : `当前筛选没有匹配的${kind}，试试全部受众或全部时间`)
        : `已加载 ${shortDramaState.items.length} 部${kind} · 继续下滑会提前补卡`;
  }
  return visible;
}
function updateShortDramaCounters(){
  const count = document.getElementById('shortDramaCount');
  if(count) count.textContent = String(shortDramaState.items.length);
  applyShortDramaFilter();
}
function shortDramaLoadingMarkup(){
  const kind = isComicDramaChannel() ? '漫剧' : '短剧';
  return `<div class="short-drama-loading" role="status" aria-live="polite"><span class="short-drama-loader-orbit" aria-hidden="true"></span><span class="short-drama-loading-copy"><b>正在读取${kind}目录</b><small>正在同步热播内容</small></span></div>`;
}
function updateShortDramaSentinel(state, extra){
  const sentinel = document.getElementById('shortDramaSentinel');
  const text = document.getElementById('shortDramaSentinelText');
  if(!sentinel || !text) return;
  sentinel.dataset.state = state;
  sentinel.classList.toggle('is-initial', state === 'initial');
  sentinel.classList.toggle('is-loading', false);
  sentinel.classList.toggle('is-error', state === 'error');
  sentinel.classList.toggle('is-hidden-feed', state !== 'error');
  sentinel.setAttribute('aria-hidden', state === 'error' ? 'false' : 'true');
  const kind = isComicDramaChannel() ? '漫剧' : '短剧';
  if(state === 'error') text.textContent = extra || '加载失败，点击重试';
  else if(state === 'wrapped') text.textContent = isComicDramaChannel() ? '本轮热播榜已刷完，正在轮换…' : '本轮内容已刷完，正在轮换新一批…';
  else text.textContent = `继续下滑加载更多${kind}`;
}
function shortDramaNextSlot(channel = shortDramaState.channel){
  return channel === 'comic' ? 'comic' : 'short';
}
function shortDramaNextBucket(channel = shortDramaState.channel){
  const slot = shortDramaNextSlot(channel);
  return shortDramaState.nextPage[slot] || (shortDramaState.nextPage[slot] = {cursor:null, facet:'', page:null, promise:null});
}
function shortDramaVisibleCardCount(){
  return document.querySelectorAll('#shortDramaGrid .short-drama-card:not(.sd-filtered-out)').length;
}
function shortDramaFeedNeedsMore(){
  const sentinel = document.getElementById('shortDramaSentinel');
  if(!sentinel || sentinel.dataset.state === 'error') return false;
  const rect = sentinel.getBoundingClientRect();
  const ahead = window.innerHeight * 1.15;
  return rect.top < window.innerHeight + ahead;
}
function applyShortDramaPage(page, channel){
  const incoming = Array.isArray(page?.items) ? page.items : [];
  if(channel === 'comic') incoming.forEach(item => { if(item) item.kind = 'comic'; });
  shortDramaState.cursor = page?.nextCursor ?? shortDramaState.cursor;
  const fresh = incoming.filter(item => item?.id && !shortDramaState.seen.has(String(item.id)));
  incoming.forEach(item => { if(item?.id) shortDramaState.seen.add(String(item.id)); });
  if(fresh.length){
    shortDramaState.allDupStreak = 0;
    shortDramaState.items.push(...fresh);
    appendShortDramaCards(fresh);
    const updated = document.getElementById('shortDramaUpdated');
    if(updated) updated.textContent = `${channel === 'comic' ? '红果漫剧' : '红果短剧'} · 更新于 ${new Date().toLocaleTimeString('zh-CN',{hour:'2-digit',minute:'2-digit'})}`;
    updateShortDramaSentinel('idle');
  }else if(incoming.length){
    shortDramaState.allDupStreak++;
    if(shortDramaState.allDupStreak >= 3){
      shortDramaState.seen.clear();
      shortDramaState.allDupStreak = 0;
      updateShortDramaSentinel('wrapped');
    }else{
      updateShortDramaSentinel('idle');
    }
  }else{
    updateShortDramaSentinel('idle');
  }
  updateShortDramaCounters();
  return fresh.length;
}
function prefetchNextShortDramaPage(){
  if(!TtvBackend.available()) return Promise.resolve(null);
  const channel = shortDramaState.channel;
  const facet = channel === 'comic' ? '' : shortDramaFacetQuery();
  const cursor = shortDramaState.cursor;
  if(!cursor) return Promise.resolve(null);
  const bucket = shortDramaNextBucket(channel);
  if(bucket.cursor === cursor && bucket.facet === facet && (bucket.page || bucket.promise)){
    return bucket.promise || Promise.resolve(bucket.page);
  }
  bucket.cursor = cursor;
  bucket.facet = facet;
  bucket.page = null;
  bucket.promise = TtvBackend.invoke(channel === 'comic' ? 'comic_drama_stream' : 'short_drama_stream', {
    input:{cursor, facet}
  }).then(page => {
    if(bucket.cursor === cursor && bucket.facet === facet) bucket.page = page;
    const items = Array.isArray(page?.items) ? page.items : [];
    if(channel === 'comic') items.forEach(item => { if(item) item.kind = 'comic'; });
    scheduleShortDramaDetailPrefetch(items, 3);
    return page;
  }).catch(() => null).finally(() => {
    if(bucket.cursor === cursor && bucket.facet === facet) bucket.promise = null;
  });
  return bucket.promise;
}
async function takeNextShortDramaPage(channel, facet, cursor){
  const bucket = shortDramaNextBucket(channel);
  if(bucket.cursor !== cursor || bucket.facet !== facet) return null;
  if(bucket.page){
    const page = bucket.page;
    bucket.page = null;
    return page;
  }
  if(bucket.promise){
    const page = await bucket.promise;
    if(bucket.page === page) bucket.page = null;
    return page || null;
  }
  return null;
}
async function loadMoreShortDrama(){
  if(shortDramaState.loading) return;
  if(!TtvBackend.available()) return renderShortDramaSeedFallback();
  const requestId = ++shortDramaState.requestId;
  const channel = shortDramaState.channel;
  const isInitialLoad = shortDramaState.items.length === 0;
  const facet = channel === 'comic' ? '' : shortDramaFacetQuery();
  shortDramaState.loading = true;
  updateShortDramaSentinel(isInitialLoad ? 'initial' : 'idle');
  try{
    const prefetched = isInitialLoad
      ? await takePrefetchedHongguoPage(channel, facet)
      : await takeNextShortDramaPage(channel, facet, shortDramaState.cursor);
    const page = prefetched || await TtvBackend.invoke(channel === 'comic' ? 'comic_drama_stream' : 'short_drama_stream', {
      input:{cursor: shortDramaState.cursor, facet}
    });
    if(requestId !== shortDramaState.requestId || shortDramaState.channel !== channel) return;
    applyShortDramaPage(page, channel);
    void prefetchNextShortDramaPage();
  }catch(error){
    if(requestId !== shortDramaState.requestId || shortDramaState.channel !== channel) return;
    shortDramaState.chain = 0;
    if(isInitialLoad) updateShortDramaSentinel('error', `${channel === 'comic' ? '漫剧' : '短剧'}目录加载失败：` + backendErrorMessage(error));
    else toast(`${channel === 'comic' ? '漫剧' : '短剧'}继续加载失败，下滑即可重试`);
  }finally{
    if(requestId === shortDramaState.requestId) shortDramaState.loading = false;
  }
  if(requestId === shortDramaState.requestId) maybeChainShortDramaLoad();
}
function maybeChainShortDramaLoad(){
  const sentinel = document.getElementById('shortDramaSentinel');
  if(!sentinel || shortDramaState.loading) return;
  if(sentinel.dataset.state === 'error') return;
  const visible = shortDramaVisibleCardCount();
  // 首屏填满后只后台预取下一页，不再把十几页卡片一次性倒进 DOM。
  if(visible >= 28 || !shortDramaFeedNeedsMore() || shortDramaState.chain >= 2){
    void prefetchNextShortDramaPage();
    return;
  }
  shortDramaState.chain++;
  window.setTimeout(() => void loadMoreShortDrama(), 48);
}
function resetShortDramaStream(){
  shortDramaState.items = [];
  shortDramaState.itemById.clear();
  shortDramaState.seen.clear();
  shortDramaState.cursor = null;
  shortDramaState.allDupStreak = 0;
  shortDramaState.chain = 0;
  shortDramaState.loading = false;
  shortDramaState.requestId++;
  shortDramaState.nextPage = {short:null, comic:null};
  const grid = document.getElementById('shortDramaGrid');
  if(grid) grid.innerHTML = shortDramaLoadingMarkup();
  updateShortDramaSentinel('initial');
  updateShortDramaCounters();
}
function refreshShortDrama(){
  resetShortDramaStream();
  void loadMoreShortDrama();
}
function applyHongguoSourceChrome(){
  const comic = isComicDramaChannel();
  document.querySelector('.short-drama-page')?.classList.toggle('is-comic', comic);
  document.querySelector('.short-drama-page')?.classList.toggle('is-unified', true);
  document.querySelectorAll('[data-hongguo-source]').forEach(button => {
    const active = button.dataset.hongguoSource === (comic ? 'comic' : 'short');
    button.classList.toggle('active', active);
    if(button.hasAttribute('aria-selected')) button.setAttribute('aria-selected', active ? 'true' : 'false');
  });
  const cap = document.getElementById('shortDramaCap');
  if(cap) cap.textContent = comic ? '已加载漫剧' : '已加载短剧';
  const note = document.getElementById('shortDramaSourceNote');
  if(note){
    note.textContent = comic
      ? '漫剧使用独立播放模型和缓存。官网网页端每部开放前几集，其余集数走 App 云端解析。'
      : '短剧使用独立播放模型和缓存。点卡片选集即可在应用内播放，无需跳转官网。';
  }
  const hint = document.getElementById('shortDramaFilterHint');
  if(hint) hint.textContent = '横向滑动查看更多题材，受众和时间可筛选当前卡片';
  const search = document.getElementById('shortDramaSearch');
  if(search) search.placeholder = comic ? '搜索漫剧' : '搜索短剧';
  document.querySelector('.short-drama-tools')?.classList.remove('is-comic-mode');
  ['shortDramaGender', 'shortDramaTime', 'shortDramaSort'].forEach(id => {
    const el = document.getElementById(id);
    if(!el) return;
    el.hidden = false;
    el.removeAttribute('hidden');
    if(id === 'shortDramaGender') el.setAttribute('aria-label', comic ? '漫剧受众' : '短剧受众');
    if(id === 'shortDramaTime') el.setAttribute('aria-label', comic ? '漫剧上新时间' : '短剧上新时间');
    if(id === 'shortDramaSort') el.setAttribute('aria-label', comic ? '漫剧排序' : '短剧排序');
  });
  const refresh = document.getElementById('shortDramaRefresh');
  if(refresh){
    const label = comic ? '刷新漫剧' : '刷新短剧';
    refresh.title = label;
    refresh.setAttribute('aria-label', label);
  }
}
function setHongguoSource(channel){
  const next = channel === 'comic' ? 'comic' : 'short';
  if(shortDramaState.channel === next && shortDramaState.started) return;
  snapshotShortDramaChannel();
  shortDramaState.channel = next;
  applyHongguoSourceChrome();
  if(restoreShortDramaChannel(next)){
    renderShortDramaCategoryChips();
    scheduleShortDramaDetailPrefetch(shortDramaState.items, 8);
    prefetchHongguoCatalog(next === 'comic' ? 'short' : 'comic');
    return;
  }
  shortDramaState.facet = '';
  shortDramaState.query = '';
  shortDramaState.gender = '';
  shortDramaState.time = '';
  shortDramaState.sort = '1';
  shortDramaState.started = false;
  const search = document.getElementById('shortDramaSearch');
  if(search) search.value = '';
  ['shortDramaGender', 'shortDramaTime', 'shortDramaSort'].forEach(id => {
    const el = document.getElementById(id);
    if(el) el.value = id === 'shortDramaSort' ? '1' : '';
  });
  renderShortDramaCategoryChips();
  resetShortDramaStream();
  shortDramaEnsureStarted();
}
function shortDramaEnsureStarted(){
  if(shortDramaState.started || shortDramaState.loading) return;
  shortDramaState.started = true;
  if(!TtvBackend.available()) return renderShortDramaSeedFallback();
  void loadMoreShortDrama();
  prefetchHongguoCatalog('short');
  prefetchHongguoCatalog('comic');
}
function renderShortDramaSeedFallback(){
  const grid = document.getElementById('shortDramaGrid');
  if(!grid) return;
  shortDramaState.items = (isComicDramaChannel() ? COMIC_DRAMA_SEED : SHORT_DRAMA_SEED).slice();
  shortDramaState.seen = new Set(shortDramaState.items.map(item => String(item.id)));
  grid.innerHTML = '';
  appendShortDramaCards(shortDramaState.items);
  updateShortDramaCounters();
  updateShortDramaSentinel('idle');
  const updated = document.getElementById('shortDramaUpdated');
  if(updated) updated.textContent = '桌面端未连接 · 展示内置精选';
  toast(`${isComicDramaChannel() ? '漫剧热播榜' : '短剧无限流'}需要 TTV 桌面端，当前展示内置精选。`);
}
function shortDramaDetailFromItem(item){
  const comic = hongguoKindOf(item) === 'comic';
  const kind = comic ? '漫剧' : '短剧';
  const totalEpisodes = Number(item?.totalEpisodes) || Number(String(item?.episodes || '').match(/\d+/)?.[0]) || 0;
  const playableEpisodes = Math.max(0, Math.min(Number(item?.playableEpisodes) || 0, totalEpisodes));
  const tags = String(item?.category || kind).split(/[\/／]/).map(value => value.trim()).filter(Boolean);
  return {
    id: String(item?.id || ''),
    title: item?.title || `${kind}详情`,
    coverUrl: item?.coverUrl || '/assets/detail-poster.jpg',
    intro: item?.description || (comic ? '来自红果漫剧热播榜的作品。' : '来自红果短剧公开目录的短剧作品。'),
    tags,
    cast: [],
    episodesText: item?.episodes || (totalEpisodes ? `全${totalEpisodes}集` : '集数待更新'),
    totalEpisodes,
    playableEpisodes,
    vids: [],
    sourceUrl: item?.sourceUrl || (comic ? 'https://hongguoduanju.com/rank/hot-comic-drama' : 'https://hongguoduanju.com/'),
    source: item?.source || (comic ? '红果漫剧' : '红果短剧官网'),
    kind: comic ? 'comic' : 'short',
    recommendations: []
  };
}
function normalizeShortDramaDetail(detail, fallback = {}){
  const merged = {...shortDramaDetailFromItem(fallback), ...(detail || {})};
  const comic = hongguoKindOf(merged) === 'comic' || hongguoKindOf(fallback) === 'comic';
  merged.kind = comic ? 'comic' : 'short';
  merged.id = String(merged.id || fallback?.id || '');
  merged.title = merged.title || fallback?.title || (comic ? '漫剧详情' : '短剧详情');
  merged.coverUrl = merged.coverUrl || fallback?.coverUrl || '/assets/detail-poster.jpg';
  merged.intro = merged.intro || fallback?.description || (comic ? '来自红果漫剧热播榜的作品。' : '来自红果短剧公开目录的短剧作品。');
  merged.tags = Array.isArray(merged.tags) && merged.tags.length
    ? merged.tags.filter(Boolean)
    : String(fallback?.category || (comic ? '漫剧' : '短剧')).split(/[\/／]/).map(value => value.trim()).filter(Boolean);
  merged.cast = Array.isArray(merged.cast) ? merged.cast : [];
  merged.vids = Array.isArray(merged.vids) ? merged.vids.map(String) : [];
  merged.totalEpisodes = Number(merged.totalEpisodes) || merged.vids.length || Number(String(merged.episodesText || fallback?.episodes || '').match(/\d+/)?.[0]) || 0;
  merged.playableEpisodes = Math.max(0, Math.min(Number(merged.playableEpisodes) || 0, merged.vids.length || merged.totalEpisodes));
  merged.episodesText = merged.episodesText || fallback?.episodes || (merged.totalEpisodes ? `全${merged.totalEpisodes}集` : '集数待更新');
  merged.sourceUrl = merged.sourceUrl || fallback?.sourceUrl || 'https://hongguoduanju.com/';
  merged.source = merged.source || fallback?.source || '红果短剧官网';
  return merged;
}
function shortDramaDetailMovie(detail){
  const normalized = normalizeShortDramaDetail(detail);
  const comic = normalized.kind === 'comic';
  const prefix = comic ? 'comicdrama' : 'shortdrama';
  const episodes = normalized.vids.map((vid, index) => ({
    id: `${prefix}:${normalized.id}:${vid}`,
    title: `第 ${index + 1} 集`,
    episodeNumber: index + 1,
    durationLabel: comic ? '漫剧' : '短剧',
    vid,
    // 保留真实播放路由所需的可用范围信息，但不再把集数渲染成“锁定”。
    locked: normalized.playableEpisodes > 0 && index + 1 > normalized.playableEpisodes
  }));
  return {
    id: `${prefix}:series:${normalized.id}`,
    shortDrama: true,
    shortDramaDetail: normalized,
    t: normalized.title,
    img: normalized.coverUrl,
    summary: normalized.intro,
    y: '',
    d: normalized.episodesText,
    r: 0,
    q: comic ? '竖屏漫剧' : '竖屏短剧',
    type: comic ? 'MOTION COMIC' : 'SHORT DRAMA',
    genre: normalized.tags.join(' · ') || (comic ? '漫剧' : '短剧'),
    genres: normalized.tags,
    network: normalized.source,
    sourceLabel: comic ? '红果漫剧' : '红果短剧',
    status: normalized.vids.length ? '可站内播放' : '可查看官方页面',
    sourceUrl: normalized.sourceUrl,
    episodes,
    episodesLoaded: true,
    versions: [{name: normalized.vids.length ? `${comic ? '红果漫剧' : '红果短剧'} · ${normalized.episodesText}` : normalized.episodesText, selected:true}],
    shortDramaRecommended: Array.isArray(normalized.recommendations) ? normalized.recommendations : []
  };
}
function fetchShortDramaDetail(item){
  const seriesId = String(item?.id || '');
  const cached = shortDramaState.detailCache.get(seriesId);
  if(cached) return Promise.resolve(cached);
  const inFlight = shortDramaState.detailInflight.get(seriesId);
  if(inFlight) return inFlight;
  if(!TtvBackend.available() || !seriesId) return Promise.resolve(normalizeShortDramaDetail(shortDramaDetailFromItem(item), item));
  const command = hongguoKindOf(item) === 'comic' ? 'comic_drama_detail' : 'short_drama_detail';
  const pending = TtvBackend.invoke(command, {input:{seriesId}})
    .then(detail => {
      const normalized = normalizeShortDramaDetail(detail, item);
      shortDramaState.detailCache.set(seriesId, normalized);
      return normalized;
    })
    .finally(() => shortDramaState.detailInflight.delete(seriesId));
  shortDramaState.detailInflight.set(seriesId, pending);
  return pending;
}
function prefetchShortDramaDetail(item){
  if(!item?.id || shortDramaState.detailCache.has(String(item.id)) || shortDramaState.detailInflight.has(String(item.id))) return;
  void fetchShortDramaDetail(item).catch(() => {});
}
function scheduleShortDramaDetailPrefetch(items, limit = 6){
  const batch = (items || []).filter(item => item?.id).slice(0, limit);
  if(!batch.length) return;
  const run = () => batch.forEach(prefetchShortDramaDetail);
  if(typeof window.requestIdleCallback === 'function') window.requestIdleCallback(run, {timeout:280});
  else window.setTimeout(run, 40);
}
function snapshotShortDramaChannel(){
  const channel = shortDramaState.channel === 'comic' ? 'comic' : 'short';
  if(!shortDramaState.items.length) return;
  shortDramaState.channelCache[channel] = {
    items: shortDramaState.items.slice(),
    seen: new Set(shortDramaState.seen),
    cursor: shortDramaState.cursor,
    facet: shortDramaState.facet,
    query: shortDramaState.query,
    gender: shortDramaState.gender,
    time: shortDramaState.time,
    sort: shortDramaState.sort
  };
}
function restoreShortDramaChannel(channel){
  const cached = shortDramaState.channelCache[channel];
  if(!cached?.items?.length) return false;
  shortDramaState.items = cached.items.slice();
  shortDramaState.seen = new Set(cached.seen);
  shortDramaState.cursor = cached.cursor;
  shortDramaState.facet = cached.facet || '';
  shortDramaState.query = cached.query || '';
  shortDramaState.gender = cached.gender || '';
  shortDramaState.time = cached.time || '';
  shortDramaState.sort = cached.sort || '1';
  shortDramaState.started = true;
  shortDramaState.loading = false;
  const grid = document.getElementById('shortDramaGrid');
  if(grid) grid.innerHTML = '';
  const cachedItems = shortDramaState.items.slice();
  appendShortDramaCards(cachedItems.slice(0, 24));
  let offset = 24;
  const pumpRestored = () => {
    if(shortDramaState.channel !== channel) return;
    if(offset >= cachedItems.length) return;
    appendShortDramaCards(cachedItems.slice(offset, offset + 24));
    offset += 24;
    if(offset < cachedItems.length) window.requestAnimationFrame(pumpRestored);
  };
  if(cachedItems.length > 24) window.requestAnimationFrame(pumpRestored);
  updateShortDramaSentinel('idle');
  updateShortDramaCounters();
  const search = document.getElementById('shortDramaSearch');
  if(search) search.value = shortDramaState.query;
  ['shortDramaGender', 'shortDramaTime', 'shortDramaSort'].forEach(id => {
    const el = document.getElementById(id);
    if(!el) return;
    if(id === 'shortDramaSort') el.value = shortDramaState.sort || '1';
    else if(id === 'shortDramaGender') el.value = shortDramaState.gender;
    else el.value = shortDramaState.time;
  });
  return true;
}
function prefetchHongguoCatalog(channel, facet = ''){
  if(!TtvBackend.available()) return Promise.resolve(null);
  const slot = channel === 'comic' ? 'comic' : 'short';
  const bucket = shortDramaState.catalogPrefetch[slot] || (shortDramaState.catalogPrefetch[slot] = {facet:'', page:null, promise:null});
  if(bucket.facet === facet && (bucket.page || bucket.promise)){
    return bucket.promise || Promise.resolve(bucket.page);
  }
  bucket.facet = facet;
  bucket.page = null;
  bucket.promise = TtvBackend.invoke(slot === 'comic' ? 'comic_drama_stream' : 'short_drama_stream', {
    input:{cursor: null, facet: slot === 'comic' ? '' : facet}
  }).then(page => {
    if(bucket.facet === facet) bucket.page = page;
    const items = Array.isArray(page?.items) ? page.items : [];
    if(slot === 'comic') items.forEach(item => { if(item) item.kind = 'comic'; });
    scheduleShortDramaDetailPrefetch(items, 3);
    return page;
  }).catch(() => null).finally(() => {
    if(bucket.facet === facet) bucket.promise = null;
  });
  return bucket.promise;
}
async function takePrefetchedHongguoPage(channel, facet = ''){
  const slot = channel === 'comic' ? 'comic' : 'short';
  const bucket = shortDramaState.catalogPrefetch[slot];
  if(!bucket || bucket.facet !== facet) return null;
  if(bucket.page){
    const page = bucket.page;
    bucket.page = null;
    return page;
  }
  if(bucket.promise){
    const page = await bucket.promise;
    if(bucket.page === page) bucket.page = null;
    return page || null;
  }
  return null;
}
function openShortDramaDetail(item, sourceEl, options = {}){
  if(!item) return;
  const seriesId = String(item.id || '');
  const requestId = ++shortDramaState.detailOpenRequestId;
  const finishOpening = () => {
    if(sourceEl){
      sourceEl.classList.remove('is-detail-loading');
      sourceEl.removeAttribute('aria-busy');
    }
  };
  const openResolvedDetail = detail => {
    if(requestId !== shortDramaState.detailOpenRequestId) return;
    const normalized = normalizeShortDramaDetail(detail, item);
    shortDramaState.detailCache.set(seriesId, normalized);
    if(!shortDramaState.facet && Array.isArray(normalized.recommendations) && normalized.recommendations.length){
      const fresh = normalized.recommendations.filter(entry => entry?.id && !shortDramaState.seen.has(String(entry.id)));
      fresh.forEach(entry => shortDramaState.seen.add(String(entry.id)));
      if(fresh.length){
        shortDramaState.items.push(...fresh);
        appendShortDramaCards(fresh);
        updateShortDramaCounters();
      }
    }
    const movie = shortDramaDetailMovie(normalized);
    openDetail(movie, sourceEl);
    warmShortDramaOpening(normalized);
    if(options.autoplay){
      const first = movie.episodes[0];
      if(first) window.setTimeout(() => playEpisode(0, null, movie), 120);
      else window.open(movie.sourceUrl, '_blank', 'noopener,noreferrer');
    }
  };
  const cached = shortDramaState.detailCache.get(seriesId);
  if(cached){
    openResolvedDetail(cached);
    return;
  }
  const inflight = shortDramaState.detailInflight.get(seriesId);
  const initialDetail = normalizeShortDramaDetail(shortDramaDetailFromItem(item), item);
  if(sourceEl){
    sourceEl.classList.add('is-detail-loading');
    sourceEl.setAttribute('aria-busy', 'true');
  }
  if(inflight){
    inflight.then(detail => {
      openResolvedDetail(detail);
      finishOpening();
    }).catch(error => {
      if(requestId === shortDramaState.detailOpenRequestId){
        toast(`${hongguoKindOf(item) === 'comic' ? '漫剧' : '短剧'}详情读取失败，请重试：` + backendErrorMessage(error));
      }
      finishOpening();
    });
    return;
  }
  if(!TtvBackend.available() || !seriesId){
    finishOpening();
    // 没有桌面端时没有后续异步补全，直接展示一次目录详情，避免卡片无响应。
    openDetail(shortDramaDetailMovie(initialDetail), sourceEl);
    if(options.autoplay){
      window.setTimeout(() => window.open(initialDetail.sourceUrl, '_blank', 'noopener,noreferrer'), 120);
    }
    return;
  }
  fetchShortDramaDetail(item)
    .then(detail => {
      openResolvedDetail(detail);
      finishOpening();
    })
    .catch(error => {
      if(requestId === shortDramaState.detailOpenRequestId){
        toast(`${hongguoKindOf(item) === 'comic' ? '漫剧' : '短剧'}详情读取失败，请重试：` + backendErrorMessage(error));
      }
      finishOpening();
    });
}
function openShortDramaFallbackModal(item, error){
  const title = escapeHtml(item.title || '短剧详情');
  const cover = escapeHtml(item.coverUrl || '/assets/detail-poster.jpg');
  const url = escapeHtml(item.sourceUrl || 'https://hongguoduanju.com/');
  const note = error ? `<p class="sd-ep-note">剧集信息读取失败：${escapeHtml(backendErrorMessage(error))}，可前往官方页面观看。</p>` : '';
  openModal(title, `<div class="short-drama-detail-modal"><img src="${cover}" alt="${title}"><div><p>${escapeHtml(item.description || '')}</p><dl><dt>集数</dt><dd>${escapeHtml(item.episodes || '—')}</dd><dt>题材</dt><dd>${escapeHtml(item.category || '短剧')}</dd></dl>${note}</div></div>`, `<a class="btn btn-accent" href="${url}" target="_blank" rel="noopener noreferrer">打开官方详情页</a><button class="btn btn-ghost" onclick="closeModal()">关闭</button>`);
}
function renderShortDramaDetailModal(detail){
  if(!detail) return;
  const title = escapeHtml(detail.title || '短剧详情');
  const cover = escapeHtml(detail.coverUrl || '/assets/detail-poster.jpg');
  const tags = Array.isArray(detail.tags) ? detail.tags.map(tag => `<span class="sd-tag">${escapeHtml(tag)}</span>`).join('') : '';
  const cast = Array.isArray(detail.cast) ? detail.cast.slice(0, 8).map(member =>
    `<span class="sd-cast">${escapeHtml(member.name)}<i>${escapeHtml(String(member.role || '').replace(/^饰\s*/, ''))}</i></span>`
  ).join('') : '';
  const vids = Array.isArray(detail.vids) ? detail.vids : [];
  const playable = Math.max(0, Math.min(Number(detail.playableEpisodes) || 0, vids.length));
  let episodesHtml;
  if(vids.length){
    // 全部集数都列出；播放时由可用播放源决定具体走网页直链还是云端解析。
    const buttons = vids.map((vid, index) => {
      return `<button class="btn sd-ep-btn" type="button" data-sd-series="${escapeHtml(detail.id)}" data-sd-vid="${escapeHtml(vid)}">${index + 1}</button>`;
    }).join('');
    episodesHtml = `<div class="sd-episodes">${buttons}</div><p class="sd-ep-note">全部 ${vids.length} 集已列出 · 点击任意集数播放，系统会自动选择可用播放源并在每集结束后连播下一集。</p>`;
  }else{
    episodesHtml = '<p class="sd-ep-note">此剧网页端暂未开放播放直链，可前往官方页面观看。</p>';
  }
  const body = `<div class="short-drama-detail-modal">
    <img src="${cover}" alt="${title}">
    <div class="sd-detail-info">
      <p class="sd-intro">${escapeHtml(detail.intro || '暂无剧情简介。')}</p>
      <div class="sd-tags">${tags}</div>
      <dl><dt>集数</dt><dd>${escapeHtml(detail.episodesText || '—')}</dd></dl>
      ${cast ? `<div class="sb-label">演员</div><div class="sd-cast-list">${cast}</div>` : ''}
      <div class="sb-label">站内播放</div>
      ${episodesHtml}
    </div>
  </div>`;
  openModal(title, body, `<button class="btn btn-accent" data-sd-play-first="${escapeHtml(detail.id)}"${vids.length ? '' : ' disabled'}>▶ 播放第 1 集</button><a class="btn btn-ghost" href="${escapeHtml(detail.sourceUrl || 'https://hongguoduanju.com/')}" target="_blank" rel="noopener noreferrer">官方页面</a><button class="btn btn-ghost" onclick="closeModal()">关闭</button>`);
  document.querySelectorAll('.sd-ep-btn').forEach(button => {
    button.addEventListener('click', () => {
      playShortDramaEpisode(button.dataset.sdSeries, button.dataset.sdVid, detail, button);
    });
  });
  document.querySelector('[data-sd-play-first]')?.addEventListener('click', event => {
    if(!vids.length) return;
    playShortDramaEpisode(detail.id, vids[0], detail, event.currentTarget);
  });
  // App-API 专辑详情（album_detail/v1）：修正锁定徽标为服务端真实状态
  // （need_unlock/disable_play），官网 H5 只有"前 N 集"粗粒度信息。静默失败。
  // 锁定态校准不阻塞详情页和首集预热，放到首屏空闲时执行，避免多个 worker 同时抢启动资源。
  const calibrate = () => void enhanceShortDramaLockBadges(detail);
  if(typeof window.requestIdleCallback === 'function') window.requestIdleCallback(calibrate, {timeout:900});
  else window.setTimeout(calibrate, 420);
}

async function enhanceShortDramaLockBadges(detail){
  if(!TtvBackend.available() || !detail?.id || !Array.isArray(detail.vids) || !detail.vids.length) return;
  try{
    const album = await TtvBackend.invoke('short_drama_app_album', {
      input:{seriesId:String(detail.id), ...hongguoAppProfile(detail)}
    });
    const byVid = new Map((album?.episodes || []).map(entry => [String(entry.vid), entry]));
    if(!byVid.size) return;
    // 仅同步标题，不再向集数按钮写入锁定图标、锁定文案或禁用状态。
    document.querySelectorAll('.sd-ep-btn').forEach(button => {
      const entry = byVid.get(String(button.dataset.sdVid));
      if(entry) button.title = entry.title || '';
      button.classList.remove('sd-ep-locked');
      delete button.dataset.sdLocked;
      button.textContent = button.textContent.replace('🔒 ', '');
    });
    const note = document.querySelector('.short-drama-detail-modal .sd-ep-note');
    if(note){
      const total = byVid.size;
      note.textContent = `全部 ${total} 集已列出（播放源已同步）· 点击任意集数开始播放，播完自动连播。`;
    }
  }catch(error){
    // 专辑接口仅作校准（未配置设备凭据/签名失败均静默），保留 H5 徽标。
    console.warn('短剧专辑锁定态校准失败：', error);
  }
}
const SHORT_DRAMA_PLAYBACK_CACHE_TTL = 8 * 60 * 1000;
const SHORT_DRAMA_NEXT_COUNTDOWN_MS = 5000;
const SHORT_DRAMA_TAIL_TRIGGER_S = 5.25;
const SHORT_DRAMA_PREFETCH_AHEAD = 2;
// 详情已打开时给 App 播放模型一个很短的抢跑窗口。命中时播放器从第一帧就有
// 最高画质和完整清晰度列表；超过该窗口仍立刻使用官网直链，不让首播停在加载态。
// 首播优先等待 App 顶档直链；它携带真实清晰度列表，避免 H5 低清直链先播放。
const SHORT_DRAMA_APP_HEAD_START_MS = 850;
function shortDramaCacheKey(detail, vid){
  const kind = (detail?.kind === 'comic') || isComicDramaChannel() ? 'comic' : 'short';
  return `${kind}:${String(vid || '')}`;
}
function readShortDramaCache(cache, key){
  const entry = cache.get(key);
  if(!entry) return null;
  if(entry.expiresAt <= Date.now()){
    cache.delete(key);
    return null;
  }
  return entry.value;
}
function cachedShortDramaRequest(cache, inflight, key, loader){
  const cached = readShortDramaCache(cache, key);
  if(cached) return Promise.resolve(cached);
  const running = inflight.get(key);
  if(running) return running;
  const pending = Promise.resolve().then(loader).then(value => {
    if(value) cache.set(key, {value, expiresAt:Date.now() + SHORT_DRAMA_PLAYBACK_CACHE_TTL});
    return value;
  }).finally(() => inflight.delete(key));
  inflight.set(key, pending);
  return pending;
}
function waitForShortDramaStream(streamPromise, timeoutMs = SHORT_DRAMA_APP_HEAD_START_MS){
  let timer = null;
  const timeout = new Promise(resolve => {
    timer = window.setTimeout(() => resolve(null), timeoutMs);
  });
  return Promise.race([
    Promise.resolve(streamPromise).then(stream => stream?.url ? stream : null).catch(() => null),
    timeout
  ]).finally(() => {
    if(timer !== null) window.clearTimeout(timer);
  });
}
function shortDramaEpisodeIndex(detail, vid){
  const vids = Array.isArray(detail?.vids) ? detail.vids : [];
  return vids.findIndex(candidate => String(candidate) === String(vid));
}
function shortDramaIsWebPlayable(detail, index){
  // 漫剧官网 H5 直链只有低清档（用户实测模糊），App 流有 1080p 多档；
  // 漫剧一律走 App 流，官网直链只留作最后兜底。
  if(hongguoKindOf(detail) === 'comic') return false;
  const vids = Array.isArray(detail?.vids) ? detail.vids : [];
  const playable = Math.max(0, Math.min(Number(detail?.playableEpisodes) || 0, vids.length));
  return index < 0 || index + 1 <= playable;
}
function ensureShortDramaBufferEl(){
  if(shortDramaBufferEl) return shortDramaBufferEl;
  let el = document.getElementById('shortDramaBufferVideo');
  if(!el){
    el = document.createElement('video');
    el.id = 'shortDramaBufferVideo';
    el.className = 'short-drama-buffer-video';
    el.muted = true;
    el.playsInline = true;
    el.preload = 'auto';
    el.setAttribute('aria-hidden', 'true');
    el.tabIndex = -1;
    (player || document.body).appendChild(el);
  }
  shortDramaBufferEl = el;
  return el;
}
function bufferShortDramaMedia(url){
  const src = String(url || '').trim();
  if(!src) return;
  const el = ensureShortDramaBufferEl();
  try{
    if(el.dataset.bufferUrl === src) return;
    el.pause();
    el.removeAttribute('src');
    el.load();
    el.dataset.bufferUrl = src;
    el.preload = 'auto';
    el.src = src;
    el.load();
  }catch(_error){ /* 缓冲节点失败不影响主播放 */ }
  try{
    void fetch(src, {mode:'no-cors', credentials:'omit', cache:'force-cache', keepalive:true}).catch(() => {});
  }catch(_error){ /* HTTP 缓存预热失败时仍保留 video preload */ }
}
function clearShortDramaBuffer(){
  if(!shortDramaBufferEl) return;
  try{
    shortDramaBufferEl.pause();
    shortDramaBufferEl.removeAttribute('src');
    shortDramaBufferEl.load();
    delete shortDramaBufferEl.dataset.bufferUrl;
  }catch(_error){ /* ignore */ }
}
function shortDramaAppReady(){
  if(shortDramaState.appStatus !== null) return Promise.resolve(shortDramaState.appStatus);
  if(shortDramaState.appStatusPromise) return shortDramaState.appStatusPromise;
  if(!TtvBackend.available()) return Promise.resolve(false);
  shortDramaState.appStatusPromise = TtvBackend.invoke('short_drama_app_status')
    .then(status => {
      shortDramaState.appStatus = Boolean(status?.configured && status?.pythonFound && status?.workerFound);
      return shortDramaState.appStatus;
    })
    .catch(() => {
      shortDramaState.appStatus = false;
      return false;
    })
    .finally(() => { shortDramaState.appStatusPromise = null; });
  return shortDramaState.appStatusPromise;
}
function loadShortDramaWebPlayback(seriesId, vid, detail){
  const key = shortDramaCacheKey(detail, vid);
  const command = detail?.kind === 'comic' ? 'comic_drama_play' : 'short_drama_play';
  return cachedShortDramaRequest(shortDramaState.webPlaybackCache, shortDramaState.webPlaybackInflight, key,
    () => TtvBackend.invoke(command, {input:{seriesId:String(seriesId), vid:String(vid)}}));
}
function loadShortDramaAppStream(vid, detail){
  const key = shortDramaCacheKey(detail, vid);
  return cachedShortDramaRequest(shortDramaState.streamPlaybackCache, shortDramaState.streamInflight, key,
    async () => {
      if(!await shortDramaAppReady()) throw new Error('短剧 App 播放模型尚未配置');
      return TtvBackend.invoke('short_drama_app_stream', {input:{vid:String(vid), ...hongguoAppProfile(detail)}});
    });
}
function resolveShortDramaEpisode(seriesId, vid, detail){
  const key = shortDramaCacheKey(detail, vid);
  return cachedShortDramaRequest(shortDramaState.resolvedPlaybackCache, shortDramaState.resolvedPlaybackInflight, key,
    () => TtvBackend.invoke('short_drama_app_resolve', {
      input:{seriesId:String(seriesId), vid:String(vid), ...hongguoAppProfile(detail)}
    })
  );
}
function shortDramaQualityVersions(stream, currentUrl = ''){
  const variants = Array.isArray(stream?.variants) ? stream.variants : [];
  const sourceVariants = (variants.length ? variants : [{
    id:'default', label:'最高', url:stream?.url || stream?.playUrl, decryptionKey:stream?.decryptionKey,
    width:stream?.width, height:stream?.height, bitrate:0
  }]).slice().sort((left, right) =>
    (Number(right?.height) || 0) - (Number(left?.height) || 0)
    || (Number(right?.bitrate) || 0) - (Number(left?.bitrate) || 0)
  );
  const seen = new Set();
  const current = String(currentUrl || '').trim();
  return sourceVariants.filter(variant => {
    const url = String(variant?.url || '').trim();
    if(!url || seen.has(url)) return false;
    seen.add(url);
    return true;
  }).map((variant, index) => {
    const height = Number(variant.height) || 0;
    const rawLabel = String(variant.label || '').trim();
    const label = (height && (!rawLabel || rawLabel === '原始画质' || rawLabel === '最高' || rawLabel === 'default'))
      ? `${height}P`
      : (rawLabel || (height ? `${height}P` : '最高'));
    const url = String(variant.url);
    return {
      quality: label,
      resolution: label,
      selected: current ? url === current : index === 0,
      __media: {
        playUrl: url,
        playHeaders: {
          'User-Agent': String(stream?.downloadUa || 'com.phoenix.read/71332'),
          'Referer': String(stream?.downloadReferer || 'https://novel.snssdk.com/')
        },
        decryptionKey: String(variant.decryptionKey || stream?.decryptionKey || '') || null,
        q: label,
        playbackQuality: label
      }
    };
  });
}
function applyShortDramaQualityVersions(movie, stream){
  const versions = shortDramaQualityVersions(stream, movie?.playUrl);
  if(!versions.length) return movie;
  movie.versions = versions;
  const selected = versions.find(version => version.selected) || versions[0];
  movie.q = String(selected?.quality || versions[0].quality || movie.q || '最高');
  movie.playbackQuality = movie.q;
  if(selectedMovie && String(selectedMovie.id) === String(movie.id)){
    selectedMovie.versions = versions;
    selectedMovie.q = movie.q;
    selectedMovie.playbackQuality = movie.q;
    const chip = document.getElementById('chipQuality');
    if(chip && !chip.dataset.userSelected) chip.textContent = movie.q;
    if(typeof renderQualityMenuOptions === 'function') renderQualityMenuOptions();
  }
  return movie;
}
async function promoteShortDramaToHighest(movie, stream, requestId){
  const versions = shortDramaQualityVersions(stream);
  if(!versions.length || !movie || requestId !== shortDramaState.playRequestId) return;
  applyShortDramaQualityVersions(movie, stream);
  const highest = versions[0];
  const active = selectedMovie;
  const chip = document.getElementById('chipQuality');
  if(!active || String(active.id) !== String(movie.id) || chip?.dataset.userSelected) return;
  const nextUrl = String(highest?.__media?.playUrl || '');
  if(!nextUrl || String(active.playUrl || '') === nextUrl){
    active.q = String(highest.quality || active.q || '最高');
    active.playbackQuality = active.q;
    if(chip) chip.textContent = active.q;
    return;
  }
  // H5 低清流已抢先出画时，App 顶档流到达后从当前进度切换一次；手选画质不覆盖。
  await saveWatchProgress();
  if(requestId !== shortDramaState.playRequestId || String(selectedMovie?.id) !== String(movie.id)) return;
  void openPlayer({
    ...active,
    ...(highest.__media || {}),
    versions,
    q:String(highest.quality || active.q || '最高'),
    playbackQuality:String(highest.quality || active.q || '最高')
  }, null, true);
}
async function upgradeShortDramaQualities(prepared, requestId){
  if(!prepared?.movie || requestId !== shortDramaState.playRequestId) return;
  try{
    const stream = prepared.kind === 'stream' ? null : await loadShortDramaAppStream(prepared.vid, prepared.detail);
    if(requestId !== shortDramaState.playRequestId) return;
    const movie = (selectedMovie && String(selectedMovie.id) === String(prepared.movie.id)) ? selectedMovie : prepared.movie;
    if(stream?.url){
      applyShortDramaQualityVersions(movie, stream);
      if(prepared.kind === 'web') await promoteShortDramaToHighest(movie, stream, requestId);
    }else if(typeof renderQualityMenuOptions === 'function'){
      renderQualityMenuOptions();
    }
  }catch(error){
    console.debug('短剧清晰度列表未能升级', error);
  }
}
function shortDramaMovieFromPlayback(playback, detail){
  const comic = (detail?.kind === 'comic') || isComicDramaChannel();
  const prefix = comic ? 'comicdrama' : 'shortdrama';
  const label = comic ? '红果漫剧' : '红果短剧';
  const episodeLabel = Number(playback.episode) > 0 ? ` 第${playback.episode}集` : '';
  const movie = {
    id: `${prefix}:${playback.seriesId}:${playback.vid}`,
    t: `${playback.seriesName || detail?.title || label}${episodeLabel}`,
    img: playback.coverUrl || detail?.coverUrl || '/assets/detail-poster.jpg',
    summary: detail?.intro || `${label} · 站内播放`,
    playUrl: playback.url,
    durationSeconds: Number(playback.durationSeconds) || 0,
    sourceLabel: label,
    q: Number(playback.height) > 0 ? `${playback.height}P` : '最高'
  };
  if(Array.isArray(playback?.variants) && playback.variants.length){
    return applyShortDramaQualityVersions(movie, playback);
  }
  return movie;
}
function shortDramaLocalMovie(resolved, detail, seriesId, vid, episode){
  const comic = (detail?.kind === 'comic') || isComicDramaChannel();
  const prefix = comic ? 'comicdrama' : 'shortdrama';
  const label = comic ? '红果漫剧' : '红果短剧';
  const height = Number(resolved?.height) || 0;
  const movie = {
    id: `${prefix}:${seriesId}:${vid}`,
    t: `${detail?.title || label} 第${episode}集`,
    img: detail?.coverUrl || '/assets/detail-poster.jpg',
    summary: detail?.intro || `${label} · 云端解析`,
    playUrl: String(resolved?.playUrl || resolved?.url || ''),
    durationSeconds: 0,
    sourceLabel: `${label} · 云端直链`,
    q: height > 0 ? `${height}P` : '最高'
  };
  return applyShortDramaQualityVersions(movie, resolved);
}
function showShortDramaOpening(detail, seriesId, vid, episode, auto){
  const comic = detail?.kind === 'comic' || isComicDramaChannel();
  const prefix = comic ? 'comicdrama' : 'shortdrama';
  const label = comic ? '红果漫剧' : '红果短剧';
  const pending = {
    id:`${prefix}:${seriesId}:${vid}`,
    t:`${detail?.title || label} 第${episode || '—'}集`,
    img:detail?.coverUrl || '/assets/detail-poster.jpg',
    summary:detail?.intro || `${label} · 正在准备播放`,
    sourceLabel:label,
    q:'最高'
  };
  syncPlayerContent(pending);
  const chip = document.getElementById('chipQuality');
  if(chip) delete chip.dataset.userSelected;
  const keepPicture = player?.classList.contains('active') && (
    player.classList.contains('native-video-live') || player.classList.contains('has-real-video')
  );
  activatePlayerShell(true, {keepPicture});
  setPlayerLoading(!keepPicture, auto ? `正在准备下一集 · 第 ${episode} 集` : `正在准备第 ${episode} 集`);
}
function prefetchShortDramaEpisode(detail, index){
  const vids = Array.isArray(detail?.vids) ? detail.vids : [];
  const vid = vids[index];
  if(!detail?.id || !vid || !TtvBackend.available()) return;
  const webPlayable = shortDramaIsWebPlayable(detail, index);
  if(webPlayable){
    void loadShortDramaWebPlayback(detail.id, vid, detail).then(playback => {
      if(playback?.url) bufferShortDramaMedia(playback.url);
    }).catch(() => {});
    void loadShortDramaAppStream(vid, detail).catch(() => {});
    return;
  }
  // 锁定集优先预热 App 多清晰度直链（短剧/漫剧同一条播放模型）。
  void loadShortDramaAppStream(vid, detail).then(stream => {
    if(stream?.url && !stream?.decryptionKey) bufferShortDramaMedia(stream.url);
  }).catch(() => {});
  void resolveShortDramaEpisode(detail.id, vid, detail).then(resolved => {
    const url = resolved?.playUrl || resolved?.url;
    if(url) bufferShortDramaMedia(url);
  }).catch(() => {});
}
function warmShortDramaOpening(detail){
  if(!detail?.vids?.length) return;
  // 旧逻辑要等到 idle（最长 500ms）才启动首集解析，用户点进详情后马上播放时
  // 基本必然错过缓存。放到当前渲染帧末尾启动，锁定态校准仍留在 idle 阶段。
  window.setTimeout(() => prefetchShortDramaEpisode(detail, 0), 0);
}
function warmNextShortDramaEpisode(context){
  if(!context?.detail || !Array.isArray(context.vids)) return;
  const nextIndex = Number.isInteger(context.currentIndex) ? context.currentIndex + 1 : Number(context.episode) || 0;
  for(let offset = 0; offset < SHORT_DRAMA_PREFETCH_AHEAD; offset++){
    const index = nextIndex + offset;
    if(index < context.vids.length) prefetchShortDramaEpisode(context.detail, index);
  }
}
function shortDramaStreamMovie(stream, detail, seriesId, vid, episode){
  const comic = (detail?.kind === 'comic') || isComicDramaChannel();
  const prefix = comic ? 'comicdrama' : 'shortdrama';
  const label = comic ? '红果漫剧' : '红果短剧';
  const height = Number(stream?.height) || 0;
  const headers = { 'User-Agent': String(stream?.downloadUa || 'com.phoenix.read/71332') };
  if(stream?.downloadReferer) headers['Referer'] = String(stream.downloadReferer);
  return applyShortDramaQualityVersions({
    id: `${prefix}:${seriesId}:${vid}`,
    t: `${detail?.title || label} 第${episode}集`,
    img: detail?.coverUrl || '/assets/detail-poster.jpg',
    summary: detail?.intro || `${label} · App 直链`,
    playUrl: String(stream.url),
    playHeaders: headers,
    decryptionKey: String(stream?.decryptionKey || '') || null,
    durationSeconds: 0,
    sourceLabel: `${label} · App 直链`,
    q: height > 0 ? `${height}P` : '最高'
  }, stream);
}
async function prepareShortDramaEpisode(seriesId, vid, detail){
  const vids = Array.isArray(detail?.vids) ? detail.vids : [];
  const index = shortDramaEpisodeIndex(detail, vid);
  const episodeLabel = index >= 0 ? index + 1 : 0;
  const playable = Math.max(0, Math.min(Number(detail?.playableEpisodes) || 0, vids.length));
  const comic = (detail?.kind === 'comic') || isComicDramaChannel();
  const webPlayable = shortDramaIsWebPlayable(detail, index);
  if(webPlayable){
    const playback = await loadShortDramaWebPlayback(seriesId, vid, detail);
    if(!playback?.url) throw new Error('未取到可播放直链');
    bufferShortDramaMedia(playback.url);
    return {
      kind:'web',
      movie: shortDramaMovieFromPlayback(playback, detail),
      vids: vids.length ? vids : (Array.isArray(playback?.vids) ? playback.vids : []),
      playable: Number(playback?.playableEpisodes) || playable,
      comic, seriesId, vid, detail, episodeLabel
    };
  }
  try{
    const stream = await loadShortDramaAppStream(vid, detail);
    if(stream?.url){
      return {
        kind:'stream',
        movie: shortDramaStreamMovie(stream, detail, seriesId, vid, episodeLabel),
        vids: vids.length ? vids : (shortDramaCtx?.vids || []),
        playable, comic, seriesId, vid, detail, episodeLabel
      };
    }
  }catch(streamError){ /* 落到下载解密管线 */ }
  try{
    const resolved = await resolveShortDramaEpisode(seriesId, vid, detail);
    if(!resolved?.playUrl) throw new Error('云端解析未返回本地文件');
    bufferShortDramaMedia(resolved.playUrl);
    return {
      kind:'local',
      cached: Boolean(resolved.cached),
      movie: shortDramaLocalMovie(resolved, detail, seriesId, vid, episodeLabel),
      vids: vids.length ? vids : (shortDramaCtx?.vids || []),
      playable, comic, seriesId, vid, detail, episodeLabel
    };
  }catch(resolveError){
    const official = await loadShortDramaWebPlayback(seriesId, vid, detail).catch(() => null);
    if(official?.url){
      bufferShortDramaMedia(official.url);
      return {
        kind:'web',
        movie: shortDramaMovieFromPlayback(official, detail),
        vids: vids.length ? vids : (official.vids || []),
        playable, comic, seriesId, vid, detail, episodeLabel
      };
    }
    throw resolveError;
  }
}
function ensurePreparedShortDramaEpisode(seriesId, vid, detail){
  const key = `${shortDramaCacheKey(detail, vid)}:${String(seriesId || '')}`;
  if(shortDramaPreparedNext?.key === key && shortDramaPreparedNext.promise){
    return shortDramaPreparedNext.promise;
  }
  const promise = prepareShortDramaEpisode(seriesId, vid, detail).then(prepared => {
    if(shortDramaPreparedNext?.key === key) shortDramaPreparedNext.prepared = prepared;
    return prepared;
  }).catch(error => {
    if(shortDramaPreparedNext?.key === key) shortDramaPreparedNext = null;
    throw error;
  });
  shortDramaPreparedNext = {key, vid:String(vid), promise, prepared:null};
  return promise;
}
function setShortDramaContext({comic, seriesId, detail, vids, vid, playable}){
  const list = Array.isArray(vids) ? vids : [];
  const matchedIndex = list.findIndex(candidate => String(candidate) === String(vid));
  const currentIndex = matchedIndex >= 0 ? matchedIndex : 0;
  shortDramaCtx = {
    kind: comic ? 'comic' : 'short', seriesId: String(seriesId), detail: detail || null,
    vids: list, currentIndex, episode: currentIndex + 1,
    playable: Number(playable) || 0
  };
  shortDramaAutoAdvanceAt = Date.now();
  shortDramaAutoAdvanceKey = '';
  warmNextShortDramaEpisode(shortDramaCtx);
  return shortDramaCtx;
}
async function playShortDramaEpisode(seriesId, vid, detail, sourceEl, opts = {}){
  if(!TtvBackend.available()) return toast('站内播放需要 TTV 桌面端。');
  if(!seriesId || !vid) return toast('缺少剧集 ID，无法播放。');
  const requestId = ++shortDramaState.playRequestId;
  const index = shortDramaEpisodeIndex(detail, vid);
  const episodeLabel = index >= 0 ? index + 1 : 0;
  const webPlayable = shortDramaIsWebPlayable(detail, index);
  const isCurrentRequest = () => requestId === shortDramaState.playRequestId;
  if(!opts.auto) clearShortDramaNextCountdown();
  if(!opts.auto) showShortDramaOpening(detail, seriesId, vid, episodeLabel, false);
  else setPlayerLoading(true, `正在进入第 ${episodeLabel} 集`);
  try{
    if(webPlayable){
      toast(opts.auto ? `自动连播 · 第 ${episodeLabel} 集` : '正在打开…');
    }else{
      toast(opts.auto
        ? `自动连播 · 第 ${episodeLabel} 集（解析中…）`
        : `第 ${episodeLabel} 集正在解析为兼容视频…`);
    }
    const prepared = await ensurePreparedShortDramaEpisode(seriesId, vid, detail);
    if(!isCurrentRequest()) return;
    if(!opts.auto && prepared.kind === 'local'){
      toast(prepared.cached ? '已使用本地缓存直接播放' : '解析完成，开始播放');
    }
    if(!isCurrentRequest()) return;
    setShortDramaContext({
      comic: prepared.comic, seriesId, detail, vid,
      vids: prepared.vids, playable: prepared.playable
    });
    void openPlayer(prepared.movie, sourceEl, true);
    void upgradeShortDramaQualities(prepared, requestId);
    clearShortDramaNextCountdown();
  }catch(error){
    if(!isCurrentRequest()) return;
    clearShortDramaNextCountdown();
    const message = backendErrorMessage(error);
    const accessBlocked = /\b403\b|CDN 拒绝当前播放会话|官方已授权会话/.test(message);
    const prefix = opts.auto ? '连播结束：' : '播放失败：';
    toast(accessBlocked
      ? `${prefix}该集当前未取得官方授权播放源，请在官方页面播放或选择已开放集。`
      : prefix + message);
    if(player?.classList.contains('active')) closePlayer();
  }
}
async function advanceShortDramaEpisode(){
  if(shortDramaAdvancing) return;
  const context = shortDramaCtx;
  if(!context) return;
  const currentIndex = Number.isInteger(context.currentIndex)
    ? context.currentIndex
    : Math.max(0, Number(context.episode) - 1);
  const nextVid = context.vids[currentIndex + 1] || null;
  if(!nextVid){
    clearShortDramaNextCountdown();
    shortDramaCtx = null;
    return toast('本剧全部集数已播完。');
  }
  shortDramaAdvancing = true;
  try{
    await playShortDramaEpisode(context.seriesId, nextVid, context.detail, null, {auto:true});
  }finally{
    shortDramaAdvancing = false;
  }
}
function clearShortDramaNextCountdown(){
  if(shortDramaNextTimer){ window.clearInterval(shortDramaNextTimer); shortDramaNextTimer = null; }
  shortDramaNextDeadline = 0;
  shortDramaNextFromTail = false;
  const notice = document.getElementById('shortDramaNextNotice');
  if(notice){ notice.classList.remove('active'); notice.setAttribute('aria-hidden', 'true'); }
}
function startShortDramaNextCountdown(durationMs = SHORT_DRAMA_NEXT_COUNTDOWN_MS, {fromEnded = false} = {}){
  const context = shortDramaCtx;
  if(!context || shortDramaAdvancing || shortDramaNextTimer) return;
  const index = Number.isInteger(context.currentIndex) ? context.currentIndex : Math.max(0, Number(context.episode) - 1);
  const nextVid = context.vids[index + 1];
  if(!nextVid) return void advanceShortDramaEpisode();
  void ensurePreparedShortDramaEpisode(context.seriesId, nextVid, context.detail).catch(() => {});
  prefetchShortDramaEpisode(context.detail, index + 2);
  const notice = document.getElementById('shortDramaNextNotice');
  const secondsEl = document.getElementById('shortDramaNextSeconds');
  const titleEl = document.getElementById('shortDramaNextTitle');
  const nextEpisode = index + 2;
  if(titleEl) titleEl.textContent = fromEnded ? '本集播放完成' : `即将播放第 ${nextEpisode} 集`;
  const render = () => {
    const seconds = Math.max(0, Math.ceil((shortDramaNextDeadline - Date.now()) / 1000));
    if(secondsEl) secondsEl.textContent = String(seconds);
    return seconds;
  };
  shortDramaNextFromTail = !fromEnded;
  shortDramaNextDeadline = Date.now() + Math.max(800, Number(durationMs) || SHORT_DRAMA_NEXT_COUNTDOWN_MS);
  if(notice){ notice.classList.add('active'); notice.setAttribute('aria-hidden', 'false'); }
  render();
  shortDramaNextTimer = window.setInterval(() => {
    if(render() > 0) return;
    if(titleEl) titleEl.textContent = `正在进入第 ${nextEpisode} 集`;
    if(shortDramaNextTimer){ window.clearInterval(shortDramaNextTimer); shortDramaNextTimer = null; }
    void advanceShortDramaEpisode();
  }, 200);
}
function maybeArmShortDramaTailCountdown(){
  const context = shortDramaCtx;
  if(!context || shortDramaAdvancing) return;
  if(!isCurrentShortDramaContextMovie(selectedMovie, context)) return;
  if(!(TOTAL > 8) || !(cur >= 0)) return;
  const remaining = TOTAL - cur;
  if(remaining > SHORT_DRAMA_TAIL_TRIGGER_S){
    if(shortDramaNextFromTail && shortDramaNextTimer) clearShortDramaNextCountdown();
    return;
  }
  if(remaining < 0.12) return;
  startShortDramaNextCountdown(Math.max(900, remaining * 1000), {fromEnded:false});
}
function maybeAutoAdvanceShortDrama(){
  const context = shortDramaCtx;
  if(!context || shortDramaAdvancing) return;
  const currentIndex = Number.isInteger(context.currentIndex)
    ? context.currentIndex
    : Math.max(0, Number(context.episode) - 1);
  const currentVid = context.vids[currentIndex];
  const prefix = context.kind === 'comic' ? 'comicdrama' : 'shortdrama';
  const expectedMediaId = `${prefix}:${context.seriesId}:${currentVid}`;
  if(!currentVid || String(selectedMovie?.id || '') !== expectedMediaId) return;
  if(shortDramaNextTimer) return;
  // HTML5/native 都可能连续上报 ended；同一会话的结束事件只处理一次。
  const key = `${playerSessionId}:${currentIndex}:${expectedMediaId}`;
  if(shortDramaAutoAdvanceKey === key) return;
  if(Date.now() - shortDramaAutoAdvanceAt < 350) return;
  shortDramaAutoAdvanceKey = key;
  startShortDramaNextCountdown(SHORT_DRAMA_NEXT_COUNTDOWN_MS, {fromEnded:true});
}
function isCurrentShortDramaContextMovie(movie, context = shortDramaCtx){
  if(!movie || !context || !Array.isArray(context.vids)) return false;
  const index = Number.isInteger(context.currentIndex) ? context.currentIndex : Math.max(0, Number(context.episode) - 1);
  const vid = context.vids[index];
  const prefix = context.kind === 'comic' ? 'comicdrama' : 'shortdrama';
  return Boolean(vid) && String(movie.id || '') === `${prefix}:${context.seriesId}:${vid}`;
}
async function recoverShortDramaNativePlayback(movie){
  const context = shortDramaCtx;
  if(!isCurrentShortDramaContextMovie(movie, context) || movie?.shortDramaStreamFallback) return false;
  const index = Number.isInteger(context.currentIndex) ? context.currentIndex : Math.max(0, Number(context.episode) - 1);
  const vid = context.vids[index];
  const episode = index + 1;
  const recoveryKey = `${playerSessionId}:${String(movie.id)}:${vid}`;
  if(shortDramaNativeRecoveryKey === recoveryKey) return true;
  shortDramaNativeRecoveryKey = recoveryKey;
  setPlayerLoading(true, '当前视频流兼容性异常，正在自动切换可播放版本…');
  try{
    await closeNativePlayback();
    if(String(selectedMovie?.id || '') !== String(movie.id)) return true;
    const publicEpisode = episode <= Math.max(0, Number(context.playable) || 0);
    if(publicEpisode){
      const playback = await loadShortDramaWebPlayback(context.seriesId, vid, context.detail);
      if(playback?.url && String(selectedMovie?.id || '') === String(movie.id)){
        const fallback = shortDramaMovieFromPlayback(playback, context.detail);
        fallback.shortDramaStreamFallback = true;
        fallback.sourceLabel = `${context.kind === 'comic' ? '红果漫剧' : '红果短剧'} · 兼容直链`;
        void openPlayer(fallback, null, true);
        return true;
      }
    }
    const resolved = await TtvBackend.invoke('short_drama_app_resolve', {
      input:{seriesId:String(context.seriesId), vid:String(vid), ...hongguoAppProfile(context.detail)}
    });
    if(!resolved?.playUrl) throw new Error('兼容解析没有返回本地视频');
    if(String(selectedMovie?.id || '') !== String(movie.id)) return true;
    const fallback = shortDramaLocalMovie(resolved, context.detail, context.seriesId, vid, episode);
    fallback.shortDramaStreamFallback = true;
    fallback.sourceLabel = `${context.kind === 'comic' ? '红果漫剧' : '红果短剧'} · 兼容播放`;
    void openPlayer(fallback, null, true);
    return true;
  }catch(error){
    if(String(selectedMovie?.id || '') === String(movie.id)){
      setPlayerLoading(false);
      toast('视频兼容切换失败：' + backendErrorMessage(error));
    }
    return false;
  }finally{
    window.setTimeout(() => {
      if(shortDramaNativeRecoveryKey === recoveryKey) shortDramaNativeRecoveryKey = '';
    }, 1200);
  }
}
const shortDramaRefresh = document.getElementById('shortDramaRefresh');
shortDramaRefresh?.addEventListener('click', () => refreshShortDrama());
// 云端解析进度：仅在最耗时的解密转存阶段提示一次，其余阶段由发起时的 toast 覆盖。
window.__TAURI__?.event?.listen?.('shortdrama://app-resolve', event => {
  const payload = event?.payload || {};
  if(payload.stage === 'transcode' && isHongguoPlaybackId(selectedMovie?.id)){
    toast('云端解析：正在解密转存为本地 mp4…');
  }
});
applyHongguoSourceChrome();
renderShortDramaCategoryChips();
prefetchHongguoCatalog('short');
prefetchHongguoCatalog('comic');
document.querySelectorAll('[data-hongguo-source]').forEach(button => {
  button.addEventListener('click', () => setHongguoSource(button.dataset.hongguoSource));
});
document.getElementById('shortDramaCategories')?.addEventListener('click', event => {
  const chip = event.target.closest('[data-short-facet]');
  if(!chip) return;
  document.querySelectorAll('#shortDramaCategories .pill').forEach(item => item.classList.remove('active'));
  chip.classList.add('active');
  shortDramaState.facet = chip.dataset.shortFacet || '';
  if(isComicDramaChannel()) applyShortDramaFilter();
  else refreshShortDrama();
});
document.getElementById('shortDramaSearch')?.addEventListener('input', event => {
  shortDramaState.query = String(event.target.value || '');
  clearTimeout(shortDramaSearchTimer);
  shortDramaSearchTimer = setTimeout(() => {
    applyShortDramaFilter();
    maybeChainShortDramaLoad();
  }, 240);
});
const shortDramaFilterChanged = () => refreshShortDrama();
document.getElementById('shortDramaGender')?.addEventListener('change', event => {
  shortDramaState.gender = String(event.target.value || '');
  if(isComicDramaChannel()) applyShortDramaFilter();
  else shortDramaFilterChanged();
});
document.getElementById('shortDramaTime')?.addEventListener('change', event => {
  shortDramaState.time = String(event.target.value || '');
  if(isComicDramaChannel()) applyShortDramaFilter();
  else shortDramaFilterChanged();
});
document.getElementById('shortDramaSort')?.addEventListener('change', event => {
  shortDramaState.sort = String(event.target.value || '1');
  if(isComicDramaChannel()){
    reorderComicDramaCards();
    applyShortDramaFilter();
  }
  else shortDramaFilterChanged();
});
document.getElementById('shortDramaSentinel')?.addEventListener('click', () => void loadMoreShortDrama());
document.getElementById('shortDramaSentinel')?.addEventListener('keydown', event => {
  if(event.key === 'Enter' || event.key === ' ') void loadMoreShortDrama();
});
if('IntersectionObserver' in window){
  const sentinel = document.getElementById('shortDramaSentinel');
  if(sentinel){
    shortDramaObserver = new IntersectionObserver(entries => {
      entries.forEach(entry => {
        if(entry.isIntersecting){
          shortDramaState.chain = 0;
          if(shortDramaState.started){
            void prefetchNextShortDramaPage();
            void loadMoreShortDrama();
          }
        }
      });
    }, {rootMargin: '640px 0px'});
    shortDramaObserver.observe(sentinel);
  }
} else {
  window.addEventListener('scroll', () => {
    const sentinel = document.getElementById('shortDramaSentinel');
    if(!sentinel) return;
    const rect = sentinel.getBoundingClientRect();
    if(rect.top < window.innerHeight + 720){
      void prefetchNextShortDramaPage();
      void loadMoreShortDrama();
    }
  }, {passive: true});
}
async function moveMediaToLibrary(media){
  if(!TtvBackend.available()) return toast('当前页面未连接桌面端。');
  const current = media.libraryId || '默认影视库';
  const target = window.prompt('输入目标影视库分类名称；留空表示默认影视库。', current);
  if(target === null) return;
  try{
    const libraryId = target.trim() || null;
    await TtvBackend.invoke('library_move', {input:{mediaId:String(media.id), libraryId}});
    await loadInitialCatalog();
    toast(libraryId ? `已移动到“${libraryId}”` : '已移动到默认影视库');
  }catch(error){
    toast('移动失败：' + backendErrorMessage(error));
  }
}
async function removeMediaFromLibrary(media){
  if(!TtvBackend.available()) return toast('当前页面未连接桌面端。');
  const confirmed = window.confirm(`仅从影视库移除“${media.t}”？\n不会删除光鸭云盘或本地磁盘中的原文件。`);
  if(!confirmed) return;
  try{
    const targetIds = [media.id, ...(media.seriesRecordIds || []), ...(media.episodes || []).map(item => item.id), ...(media.versions || []).map(item => item.__media?.id)].filter(Boolean);
    for(const targetId of new Set(targetIds.map(String))){
      await TtvBackend.invoke('library_delete', {input:{mediaId:String(targetId)}});
    }
    MOVIES = MOVIES.filter(item => String(item.id) !== String(media.id));
    selectedMovie = MOVIES[0] || null;
    detailMovie = selectedMovie;
    renderGrid();
    updateCatalogChrome();
    toast('已从影视库移除，原文件未删除。');
  }catch(error){
    toast('移除失败：' + backendErrorMessage(error));
  }
}
async function moveSourceToLibrary(sourceKey){
  if(!TtvBackend.available()) return toast('当前页面未连接桌面端。');
  const items = librarySourceMedia(sourceKey);
  if(!items.length) return toast('该来源当前没有可移动的影视。');
  const current = items[0].libraryId || '默认影视库';
  const target = window.prompt(`将“${librarySourceName(sourceKey)}”中的 ${items.length} 条媒体移动到哪个影视库？`, current);
  if(target === null) return;
  const libraryId = target.trim() || null;
  try{
    for(const media of items){
      await TtvBackend.invoke('library_move', {input:{mediaId:String(media.id), libraryId}});
    }
    activeSourceFilter = 'all';
    await loadInitialCatalog();
    toast(libraryId ? `已将 ${items.length} 条媒体移动到“${libraryId}”` : `已将 ${items.length} 条媒体移动到默认影视库`);
  }catch(error){
    toast('来源移动失败：' + backendErrorMessage(error));
  }
}
function collectLibraryMediaIds(media){
  const ids = new Set([media.id]);
  (media.seriesRecordIds || []).forEach(id => ids.add(String(id)));
  (media.episodes || []).forEach(episode => episode?.id && ids.add(String(episode.id)));
  (media.versions || []).forEach(version => version?.__media?.id && ids.add(String(version.__media.id)));
  return [...ids];
}

async function deleteLibraryMediaIds(ids){
  let removed = 0;
  for(const mediaId of ids){
    if(await TtvBackend.invoke('library_delete', {input:{mediaId:String(mediaId)}})) removed++;
  }
  return removed;
}

/* ================= 来源删除进度监控 ================= */
// 后端在后台线程里分批删除来源媒体，并 emit `library://source-delete-progress`
//（phase: count → db → covers → done）。这张卡片实时显示阶段与计数；
// 页面在删除中途被刷新时本页没有发起删除，收到 done 事件就自动重载
// 媒体库，把删除后的真实数据同步回来，无需用户再手动刷新。
let sourceDeleteCard = null;
let sourceDeleteActiveCount = 0;
const SOURCE_DELETE_PHASE_LABELS = {
  count: '正在统计待删除的媒体记录…',
  db: '正在删除数据库记录',
  covers: '正在清理生成的封面文件',
  done: '删除完成'
};
function ensureSourceDeleteCard(){
  if(sourceDeleteCard?.isConnected) return sourceDeleteCard;
  sourceDeleteCard = document.createElement('div');
  sourceDeleteCard.className = 'source-delete-progress';
  sourceDeleteCard.innerHTML = `
    <div class="sdp-title">正在删除来源…</div>
    <div class="sdp-bar"><i></i></div>
    <div class="sdp-phase">准备中…</div>`;
  document.body.appendChild(sourceDeleteCard);
  return sourceDeleteCard;
}
function setSourceDeleteCardVisible(visible){
  ensureSourceDeleteCard().classList.toggle('show', Boolean(visible));
}
function setSourceDeleteBar(percent){
  const fill = ensureSourceDeleteCard().querySelector('.sdp-bar > i');
  if(!fill) return;
  if(percent === null){
    fill.classList.add('indeterminate');
    fill.style.width = '';
  }else{
    fill.classList.remove('indeterminate');
    fill.style.width = Math.min(100, Math.max(0, percent)) + '%';
  }
}
function beginSourceDeleteProgress(sourceName){
  sourceDeleteActiveCount++;
  const card = ensureSourceDeleteCard();
  card.querySelector('.sdp-title').textContent = `正在删除“${sourceName}”…`;
  card.querySelector('.sdp-phase').textContent = '正在停止来源任务…';
  setSourceDeleteBar(null);
  setSourceDeleteCardVisible(true);
}
function endSourceDeleteProgress(){
  sourceDeleteActiveCount = Math.max(0, sourceDeleteActiveCount - 1);
  if(!sourceDeleteActiveCount){
    setTimeout(() => { if(!sourceDeleteActiveCount) setSourceDeleteCardVisible(false); }, 900);
  }
}
function updateSourceDeleteProgress(payload){
  const phase = String(payload?.phase || '');
  const processed = Number(payload?.processed || 0);
  const total = Number(payload?.total || 0);
  const card = ensureSourceDeleteCard();
  const label = SOURCE_DELETE_PHASE_LABELS[phase] || phase;
  const counts = total ? ` ${processed.toLocaleString('zh-CN')} / ${total.toLocaleString('zh-CN')}` : '';
  const phaseEl = card.querySelector('.sdp-phase');
  if(phaseEl) phaseEl.textContent = label + counts;
  setSourceDeleteBar(total > 0 ? processed / total * 100 : (phase === 'done' ? 100 : null));
  if(phase === 'done' && !sourceDeleteActiveCount){
    // 删除发起页已经不在了（用户在删除中途刷新过页面）。
    const titleEl = card.querySelector('.sdp-title');
    if(titleEl) titleEl.textContent = '来源删除完成';
    toast(`后台来源删除完成，共删除 ${processed.toLocaleString('zh-CN')} 条媒体记录。`);
    setTimeout(() => { if(!sourceDeleteActiveCount) setSourceDeleteCardVisible(false); }, 1200);
    Promise.resolve(loadInitialCatalog()).catch(() => {});
  }
}
function setupSourceDeleteProgressListener(){
  const listen = window.__TAURI__?.event?.listen;
  if(typeof listen !== 'function') return;
  listen('library://source-delete-progress', (event) => {
    const payload = event && event.payload;
    if(payload) updateSourceDeleteProgress(payload);
  }).catch(() => {});
}

async function removeSourceFromLibrary(sourceKey){
  if(!TtvBackend.available()) return toast('当前页面未连接桌面端。');
  const items = librarySourceMedia(sourceKey);
  if(!items.length) return toast('该来源当前没有可移除的影视。');
  const confirmed = window.confirm(`删除来源“${librarySourceName(sourceKey)}”的 ${items.length} 条媒体记录、元数据和生成的封面？\n云盘或本地磁盘中的原始视频文件会保留。`);
  if(!confirmed) return;
  const previousMovies = MOVIES.slice();
  const previousFilter = activeSourceFilter;
  // Remove the source from the visible UI immediately. The backend cleanup can
  // involve a large SQLite delete and cover-file cleanup, so waiting for it here
  // makes the desktop webview appear frozen.
  MOVIES = MOVIES.filter(media => librarySourceKey(media) !== sourceKey);
  activeSourceFilter = 'all';
  selectedMovie = MOVIES[0] || null;
  detailMovie = selectedMovie;
  renderGrid();
  updateCatalogChrome();
  toast(`正在停止“${librarySourceName(sourceKey)}”任务并清理媒体…`);
  beginSourceDeleteProgress(librarySourceName(sourceKey));
  try{
    // 删除必须用记录里真实的 source_type（provider:guangya / openlist / local 等）。
    // 来源面板的键是显示名，仅靠名称映射回 source_type 一旦漏掉某个名字，
    // DELETE 就匹配 0 行：UI 已被乐观清空但刷新后记录全部回来。
    const sourceTypes = [...new Set(items.map(media => String(media.record?.sourceType || media.v || '').trim()).filter(Boolean))];
    if(!sourceTypes.length) sourceTypes.push(backendSourceKeyForLibrarySource(sourceKey));
    let removed = 0;
    for(const sourceType of sourceTypes){
      removed += Number(await TtvBackend.invoke('library_delete_source', {
        input:{sourceType}
      }) || 0);
    }
    if(!removed){
      throw new Error('数据库中没有匹配到该来源的记录（source_type: ' + sourceTypes.join(', ') + '）');
    }
    toast(`已停止来源任务并删除“${librarySourceName(sourceKey)}”的 ${removed} 条媒体记录、元数据和生成封面。`);
  }catch(error){
    MOVIES = previousMovies;
    activeSourceFilter = previousFilter;
    selectedMovie = MOVIES[0] || null;
    detailMovie = selectedMovie;
    renderGrid();
    updateCatalogChrome();
    toast('来源移除失败：' + backendErrorMessage(error));
  }finally{
    endSourceDeleteProgress();
  }
}
function pickYear(v){
  activeYear = v;
  document.getElementById('yearLabel').textContent = v;
  document.getElementById('yearDd').classList.remove('open');
  renderGrid();
  toast(v === '年份' ? '已显示全部年份' : '筛选年份：' + v);
}
function toggleDd(id){ document.getElementById(id).classList.toggle('open'); }
document.addEventListener('click', e => {
  document.querySelectorAll('.dropdown.open').forEach(d => { if(!d.contains(e.target)) d.classList.remove('open'); });
});

/* ================= 搜索与排序 ================= */
const searchInput = document.getElementById('librarySearch');
const sortSelect = document.getElementById('librarySort');
if(searchInput){
  searchInput.addEventListener('input', e => {
    searchTerm = e.target.value.toLowerCase().trim();
    renderGrid();
  });
}
if(sortSelect){
  sortSelect.addEventListener('change', e => {
    sortMode = e.target.value;
    renderGrid();
  });
}
/* ================= 详情页扩充 ================= */
let isDetailFaved = false;
function toggleDetailFav(){
  isDetailFaved = !isDetailFaved;
  const btn = document.getElementById('dFavBtn');
  const txt = document.getElementById('dFavText');
  if(btn && txt){
    btn.style.color = isDetailFaved ? '#ff5f6b' : '';
    btn.style.borderColor = isDetailFaved ? 'rgba(255,95,107,.4)' : '';
    txt.textContent = isDetailFaved ? '已收藏' : '收藏';
  }
  persistFavorite(selectedMovie, isDetailFaved);
  toast(isDetailFaved ? '已将《' + selectedMovie.t + '》加入收藏' : '已取消收藏');
}

async function toggleFavorite(movie){
  if(!movie) return;
  const key = String(movie.id);
  const favorite = !favoriteIds.has(key);
  const targetIds = Array.isArray(movie.seriesRecordIds) && movie.seriesRecordIds.length
    ? movie.seriesRecordIds.map(String)
    : [key];
  if(isNativeMediaMode()){
    try{
      for(const mediaId of targetIds){
        await TtvBackend.invoke('favorites_toggle', {mediaId, favorite});
      }
    }catch(error){
      toast('收藏未保存：' + backendErrorMessage(error));
      return;
    }
  }
  if(favorite) favoriteIds.add(key); else favoriteIds.delete(key);
  if(selectedMovie && String(selectedMovie.id) === key) isDetailFaved = favorite;
  if(!isNativeMediaMode()) localStorage.setItem(CATALOG_FAVORITES_KEY, JSON.stringify([...favoriteIds]));
  renderWatchlist();
  renderGrid();
  toast(favorite ? '已加入收藏' : '已取消收藏');
}
function persistFavorite(movie, favorite){
  if(!movie) return;
  const key = String(movie.id);
  const targetIds = Array.isArray(movie.seriesRecordIds) && movie.seriesRecordIds.length
    ? movie.seriesRecordIds.map(String)
    : [key];
  if(favorite) favoriteIds.add(key); else favoriteIds.delete(key);
  if(isNativeMediaMode()){
    Promise.all(targetIds.map(mediaId => TtvBackend.invoke('favorites_toggle', {mediaId, favorite})))
      .then(() => { renderWatchlist(); })
      .catch(error => toast('收藏未保存：' + backendErrorMessage(error)));
  }else{
    localStorage.setItem(CATALOG_FAVORITES_KEY, JSON.stringify([...favoriteIds]));
    renderWatchlist();
  }
}

function episodeShortLabel(episode){
  if(episode?.seasonNumber && episode?.episodeNumber){
    return 'S' + String(episode.seasonNumber).padStart(2, '0') + 'E' + String(episode.episodeNumber).padStart(2, '0');
  }
  if(episode?.episodeNumber) return '第 ' + String(episode.episodeNumber).padStart(2, '0') + ' 集';
  return episode?.title || '剧集';
}
function createEpisodePlaybackMovie(parentMovie, episode, episodeIndex){
  const seriesTitle = parentMovie?.seriesTitle || parentMovie?.t || '影视';
  const label = episodeShortLabel(episode);
  const episodeTitle = episode?.title && episode.title !== label ? label + ' · ' + episode.title : label;
  return {
    ...parentMovie,
    id: episode.id,
    providerId: episode.providerId || parentMovie?.providerId,
    providerMediaId: episode.providerMediaId,
    playUrl: episode.playUrl || '',
    browserPlayUrl: '',
    playHeaders: episode.playHeaders || {},
    t: seriesTitle + ' · ' + episodeTitle,
    d: episode.durationLabel || formatDuration(episode.durationSeconds),
    durationSeconds: episode.durationSeconds || 0,
    img: episode.img || parentMovie?.img,
    summary: episode.summary || parentMovie?.summary,
    type: 'episode',
    episodes: parentMovie?.episodes || [],
    episodesLoaded: true,
    episodeIndex,
    episodeNumber: episode.episodeNumber || 0,
    seasonNumber: episode.seasonNumber || 0,
    seriesId: parentMovie?.seriesId || parentMovie?.id,
    seriesTitle,
    seriesProviderMediaId: parentMovie?.seriesProviderMediaId || parentMovie?.providerMediaId
  };
}
function activeEpisodeIndex(episodes){
  if(!Array.isArray(episodes) || !selectedMovie) return -1;
  if(Number.isInteger(selectedMovie.episodeIndex) && episodes[selectedMovie.episodeIndex]) return selectedMovie.episodeIndex;
  return episodes.findIndex(episode =>
    String(episode.id) === String(selectedMovie.id) ||
    (episode.providerMediaId && episode.providerMediaId === selectedMovie.providerMediaId)
  );
}
function renderDetailEpisodes(movie, state = {}){
  const section = document.getElementById('detailEpisodesSection');
  const grid = document.getElementById('episodesGrid');
  const title = document.getElementById('detailEpisodesTitle');
  if(!section || !grid) return;
  const episodes = Array.isArray(movie?.episodes) ? movie.episodes : [];
  const shouldShow = state.loading || Boolean(state.error) || episodes.length > 0;
  section.hidden = !shouldShow;
  grid.innerHTML = '';
  if(!shouldShow) return;
  if(title) title.textContent = episodes.length ? '剧集选集 · ' + episodes.length + ' 集' : '剧集选集';
  if(state.loading){
    grid.innerHTML = '<div class="episodes-state">正在读取真实剧集数据...</div>';
    return;
  }
  if(state.error){
    grid.innerHTML = '<div class="episodes-state">' + escapeHtml(state.error) + '</div>';
    return;
  }
  const activeIndex = activeEpisodeIndex(episodes);
  const shortDramaLimit = movie?.shortDrama ? 24 : episodes.length;
  const expanded = Boolean(movie?.shortDrama && grid.dataset.sdEpExpanded === '1');
  const visibleEpisodes = movie?.shortDrama && !expanded && episodes.length > shortDramaLimit
    ? episodes.slice(0, shortDramaLimit)
    : episodes;
  grid.classList.toggle('is-expanded', expanded);
  grid.dataset.sdEpExpanded = expanded ? '1' : '0';
  const appendEpisodeButton = (episode, index) => {
    const button = document.createElement('button');
    button.className = 'ep-btn' + (index === activeIndex ? ' active' : '');
    button.textContent = episodeShortLabel(episode);
    button.title = episode.title || button.textContent;
    button.addEventListener('click', () => playEpisode(index, button, movie));
    grid.appendChild(button);
  };
  visibleEpisodes.forEach(appendEpisodeButton);
  if(movie?.shortDrama && episodes.length > shortDramaLimit){
    const more = document.createElement('button');
    more.type = 'button';
    more.className = 'ep-btn episodes-more-btn' + (expanded ? ' is-collapse' : '');
    more.textContent = expanded ? '收起' : `更多 ${episodes.length - shortDramaLimit}`;
    more.title = expanded ? '收起选集' : `展开第 ${shortDramaLimit + 1} - ${episodes.length} 集`;
    more.setAttribute('aria-label', more.title);
    more.addEventListener('click', () => {
      grid.dataset.sdEpExpanded = expanded ? '0' : '1';
      renderDetailEpisodes(movie, state);
      if(!expanded) grid.scrollTop = 0;
    });
    grid.appendChild(more);
  }
}
async function ensureMovieEpisodes(movie){
  if(!movie) return [];
  if(movie.episodesLoaded) return Array.isArray(movie.episodes) ? movie.episodes : [];
  if(movie.episodesPromise) return movie.episodesPromise;
  if(!isStreamHubShow(movie) || !TtvBackend.available()){
    movie.episodes = Array.isArray(movie.episodes) ? movie.episodes : [];
    movie.episodesLoaded = true;
    return movie.episodes;
  }
  movie.episodesPromise = (async () => {
    const files = [];
    const seenTokens = new Set();
    let pageToken = null;
    for(let pageIndex = 0; pageIndex < 20; pageIndex++){
      const page = await TtvBackend.invoke('provider_list_files', {
        providerId: movie.providerId,
        input: {parentId: movie.providerMediaId, pageSize: 500, ...(pageToken ? {pageToken} : {})}
      });
      if(Array.isArray(page?.files)) files.push(...page.files);
      const next = page?.nextPageToken || null;
      if(!next || seenTokens.has(next)) break;
      seenTokens.add(next);
      pageToken = next;
    }
    movie.episodes = normalizeEpisodes(files, movie);
    movie.episodesLoaded = true;
    return movie.episodes;
  })();
  try{
    return await movie.episodesPromise;
  }finally{
    movie.episodesPromise = null;
  }
}
function playEpisode(episodeIndex, btn, parentMovie){
  const parent = parentMovie || (selectedMovie?.episodes?.length ? selectedMovie : detailMovie);
  const episodes = Array.isArray(parent?.episodes) ? parent.episodes : [];
  const episode = episodes[episodeIndex];
  if(parent?.shortDrama && episode?.vid){
    if(btn){
      btn.parentElement?.querySelectorAll('.ep-btn').forEach(item => item.classList.remove('active'));
      btn.classList.add('active');
    }
    void playShortDramaEpisode(parent.shortDramaDetail?.id, episode.vid, parent.shortDramaDetail, btn || null);
    return;
  }
  if(!episode || (!episode.providerMediaId && !episode.playUrl)){
    toast('该集没有绑定真实播放资源。');
    return;
  }
  if(btn){
    btn.parentElement?.querySelectorAll('.ep-btn').forEach(item => item.classList.remove('active'));
    btn.classList.add('active');
  }
  const playbackMovie = createEpisodePlaybackMovie(parent, episode, episodeIndex);
  void openPlayer(playbackMovie, btn || null);
  toast('正在播放：' + playbackMovie.t);
}

function pickVersion(el, verName){
  el.parentElement.querySelectorAll('.version-item').forEach(v => v.classList.remove('active'));
  el.classList.add('active');
  toast('已切换至版本：' + verName);
}

function isComicDramaMovie(movie){
  return movie?.shortDramaDetail?.kind === 'comic' || String(movie?.id || '').startsWith('comicdrama:');
}
function detailSourceLabel(movie){
  if(!movie) return '媒体库';
  if(movie.shortDrama) return isComicDramaMovie(movie) ? '红果漫剧' : '红果短剧';
  if(movie.providerId === 'streamhub') return 'StreamHub';
  if(movie.providerId) return SOURCE_CATALOG.find(item => item.id === movie.providerId)?.name || movie.providerId;
  if(appMode === 'catalog') return 'TVMaze';
  return movie.sourceLabel || (movie.playUrl ? '本地文件' : '媒体库');
}
function basename(value){
  const raw = String(value || '').trim();
  if(!raw) return '';
  if(/^https?:\/\//i.test(raw)){
    try{
      const last = new URL(raw).pathname.split('/').filter(Boolean).pop() || '';
      return /\.(avi|flv|m2ts|m4v|mkv|mov|mp4|mpeg|mpg|rm|rmvb|ts|webm|wmv)$/i.test(last)
        ? decodeURIComponent(last)
        : '';
    }catch(error){ return ''; }
  }
  return raw.split(/[\\/]/).pop().split(/[?#]/)[0] || '';
}
function versionDetails(movie, version){
  const details = [];
  const size = positiveNumber(version?.sizeBytes ?? version?.fileSize ?? version?.size);
  const codec = version?.codec || version?.videoCodec || version?.codecName;
  const audio = version?.audioCodec || version?.audio || version?.channels;
  const duration = positiveNumber(version?.durationSeconds || movie?.durationSeconds);
  if(size) details.push(formatBytes(size));
  if(codec) details.push(String(codec).toUpperCase());
  if(audio) details.push(String(audio));
  if(duration) details.push(formatDuration(duration));
  const path = basename(version?.path || version?.fileName || movie?.playUrl);
  if(path && !/^https?:/i.test(path)) details.push(path);
  return details.join(' · ') || (movie?.sourceLabel || '已绑定真实媒体资源');
}
function renderDetailVersions(movie){
  const grid = document.getElementById('detailVersions');
  if(!grid) return;
  grid.innerHTML = '';
  const rawVersions = Array.isArray(movie?.versions) ? movie.versions : [];
  const playable = Boolean(movie?.playUrl || movie?.providerId);
  const episodes = Array.isArray(movie?.episodes) ? movie.episodes : [];
  const versions = rawVersions.length ? rawVersions : (playable ? [{name: movie.q || movie.type || '原始资源', selected: true}] : []);
  versions.forEach((version, index) => {
    const card = document.createElement('button');
    card.type = 'button';
    card.className = 'version-item' + (version?.selected || index === 0 ? ' active' : '');
    const quality = version?.quality || version?.resolution || version?.name || movie.q || '原始资源';
    const info = document.createElement('span');
    info.className = 'version-info';
    const title = document.createElement('b');
    title.textContent = String(quality);
    const description = document.createElement('small');
    description.textContent = versionDetails(movie, version);
    const badge = document.createElement('span');
    badge.className = 'badge' + (version?.selected || index === 0 ? ' badge-pro' : '');
    badge.textContent = detailSourceLabel(movie);
    info.append(title, description);
    card.append(info, badge);
    card.addEventListener('click', () => {
      card.parentElement?.querySelectorAll('.version-item').forEach(item => item.classList.remove('active'));
      card.classList.add('active');
      if(movie?.shortDrama){
        const first = movie.episodes?.[0];
        if(first?.vid) void playShortDramaEpisode(movie.shortDramaDetail?.id, first.vid, movie.shortDramaDetail, card);
        else window.open(movie.sourceUrl, '_blank', 'noopener,noreferrer');
      }else{
        void openPlayer({...movie, ...(version?.__media || {}), playbackQuality: String(quality)}, card);
      }
    });
    grid.appendChild(card);
  });
  // 短剧/漫剧已经在上方展示完整选集，这里不再额外生成“XX 集可播放”快捷卡，
  // 避免播放源卡片和选集快捷卡看起来重复。
  if(episodes.length && !movie?.shortDrama){
    const card = document.createElement('button');
    card.type = 'button';
    card.className = 'version-item';
    card.innerHTML = `<span class="version-info"><b>${escapeHtml(episodes.length + ' 集可播放')}</b><small>从上方选集列表选择具体剧集</small></span><span class="badge">选集</span>`;
    card.addEventListener('click', () => document.getElementById('detailEpisodesSection')?.scrollIntoView({behavior:'smooth', block:'center'}));
    grid.appendChild(card);
  }
  if(!grid.children.length){
    const empty = document.createElement('div');
    empty.className = 'version-empty';
    empty.innerHTML = '<b>暂无可播放资源</b><span>当前条目只有公开元数据；扫描本地目录或连接媒体来源后才会显示真实版本。</span>';
    grid.appendChild(empty);
  }
}

function formatPlaybackClock(seconds){
  const total = Math.max(0, Math.round(Number(seconds) || 0));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  return (hours ? hours + ':' : '') + String(minutes).padStart(hours ? 2 : 1, '0') + ':' + String(secs).padStart(2, '0');
}
async function openStatsModal(){
  const movie = selectedMovie;
  if(!movie) return;
  let history = null;
  if(TtvBackend.available()){
    try{ history = await TtvBackend.invoke('history_get', {mediaId: String(movie.id)}); }
    catch(error){ console.warn('Unable to load playback history:', error); }
  }
  const duration = positiveNumber(history?.durationSeconds || movie.durationSeconds);
  const position = positiveNumber(history?.positionSeconds);
  const progress = duration ? Math.min(100, Math.round(position / duration * 100)) : 0;
  const watchedAt = history?.watchedAt ? new Date(history.watchedAt).toLocaleString('zh-CN', {hour12:false}) : '暂无播放记录';
  openModal(
    '观看统计 · ' + movie.t,
    `<div class="detail-stat-grid">
      <div><span>最近进度</span><b>${history ? progress + '%' : '—'}</b></div>
      <div><span>播放位置</span><b>${history ? escapeHtml(formatPlaybackClock(position)) : '—'}</b></div>
      <div><span>媒体时长</span><b>${duration ? escapeHtml(formatPlaybackClock(duration)) : '未知'}</b></div>
      <div><span>播放状态</span><b>${history?.completed ? '已看完' : (history ? '未看完' : '未播放')}</b></div>
    </div><p class="detail-stat-time">最近播放：${escapeHtml(watchedAt)}</p>`,
    '<button class="btn btn-accent" onclick="closeModal()">完成</button>'
  );
}

function renderSimilarMovies(currentMovie){
  const container = document.getElementById('similarRow');
  const title = document.getElementById('similarSectionTitle');
  if(!container) return;
  container.innerHTML = '';
  if(currentMovie?.shortDrama){
    const comic = isComicDramaMovie(currentMovie);
    const kind = comic ? '漫剧' : '短剧';
    if(title) title.textContent = `更多红果${kind}`;
    const seen = new Set([String(currentMovie.shortDramaDetail?.id || '')]);
    const pool = [
      ...(Array.isArray(currentMovie.shortDramaRecommended) ? currentMovie.shortDramaRecommended : []),
      ...shortDramaState.items
    ].filter(item => item?.id && !seen.has(String(item.id)) && (seen.add(String(item.id)), true)).slice(0, 8);
    if(!pool.length){
      container.innerHTML = `<p class="catalog-empty">正在准备更多${kind}内容。</p>`;
      return;
    }
    pool.forEach(item => {
      const card = document.createElement('div');
      card.className = 'similar-card short-drama-similar-card';
      const image = document.createElement('img');
      image.alt = item.title || kind;
      image.dataset.coverSrc = item.coverUrl || '/assets/detail-poster.jpg';
      bindCardCover(image);
      const caption = document.createElement('div');
      caption.className = 'similar-title';
      caption.textContent = item.title || kind;
      card.append(image, caption);
      card.addEventListener('click', () => openShortDramaDetail(item, card));
      container.appendChild(card);
    });
    return;
  }
  const adult = Boolean(currentMovie && currentMovie.adult);
  if(title) title.textContent = adult ? '深夜档的其他内容' : '媒体库中的其他内容';
  const currentActors = new Set(adultActors(currentMovie));
  const pool = MOVIES.filter(m => m.id !== (currentMovie ? currentMovie.id : 0) && Boolean(m.adult) === adult);
  const ranked = pool.map(m => {
    let score = 0;
    if(adult){
      const shared = adultActors(m).filter(actor => currentActors.has(actor)).length;
      score += shared * 8;
      if(adultStudio(m) && adultStudio(m) === adultStudio(currentMovie)) score += 3;
      if(adultSeries(m) && adultSeries(m) === adultSeries(currentMovie)) score += 4;
      if(adultTags(m).some(tag => adultTags(currentMovie).includes(tag))) score += 1;
    }
    if(m.hasArtwork) score += 1;
    return {m, score};
  }).sort((a, b) => b.score - a.score || String(b.m.y || '').localeCompare(String(a.m.y || '')));
  const similars = ranked.slice(0, 8).map(item => item.m);
  if(!similars.length){
    container.innerHTML = `<p class="catalog-empty">${adult ? '深夜档里暂时没有其他可关联的作品。' : '媒体库里暂时没有其他内容。'}</p>`;
    return;
  }
  similars.forEach(sm => {
    const card = document.createElement('div');
    card.className = 'similar-card' + (adult ? ' adult-cover' : '');
    const cover = normalizeArtworkUrl(sm.img || sm.artRemote || '', '/assets/detail-poster.jpg');
    const caption = adult && adultCode(sm) ? `${adultCode(sm)} ${sm.t}` : sm.t;
    const image = document.createElement('img');
    image.alt = caption;
    image.dataset.coverSrc = cover;
    bindCardCover(image, {owner: sm});
    const captionEl = document.createElement('div');
    captionEl.className = 'similar-title';
    captionEl.textContent = caption;
    card.appendChild(image);
    card.appendChild(captionEl);
    card.addEventListener('click', () => {
      openDetail(sm, card);
      window.scrollTo({top: 0, behavior: 'smooth'});
    });
    container.appendChild(card);
  });
}

let isDetailMorphing = false;
function openDetail(m, sourceEl){
  if(!m) return;
  const isDetailSwap = currentView === 'detail';
  // 记录详情页的返回目标：深夜档和短剧分别回到各自内容页，其余返回媒体库。
  if(currentView !== 'detail') detailReturnView = currentView === 'adult' ? 'adult' : (currentView === 'short-drama' ? 'short-drama' : 'library');
  const backLink = document.querySelector('#view-detail .back-link');
  if(backLink) backLink.lastChild.textContent = detailReturnView === 'adult'
    ? ' 返回深夜档'
    : (detailReturnView === 'short-drama' ? ` 返回${isComicDramaMovie(m) ? '漫剧' : '短剧'}` : ' 返回媒体库');
  detailMovie = m;
  selectedMovie = m;

  const dPoster = document.getElementById('dPoster');
  const dPosterCard = document.querySelector('.d-poster-card');
  const viewDetail = document.getElementById('view-detail');
  if(viewDetail) viewDetail.classList.toggle('short-drama-detail-view', Boolean(m.shortDrama));
  
  const posterFrame = document.querySelector('.d-poster');
  if(posterFrame) posterFrame.classList.toggle('adult-cover', Boolean(m.adult));
  if(dPoster){
    delete dPoster.dataset.artRemoteTried;
    delete dPoster.dataset.artFallback;
    dPoster.src = normalizeArtworkUrl(m.img || m.artRemote || '', '/assets/detail-poster.jpg');
    dPoster.alt = m.t || '媒体海报';
    attachArtworkFallback(dPoster, '/assets/detail-poster.jpg', m);
  }
  const adultTitle = m.adult && adultCode(m) ? `${adultCode(m)} ${toSimplifiedZh(m.t)}` : toSimplifiedZh(m.t);
  const dTitle = document.getElementById('dTitle');
  if(dTitle) dTitle.textContent = adultTitle || '';
  const ratingEl = document.getElementById('dRate');
  if(ratingEl) ratingEl.textContent = m.r ? Number(m.r).toFixed(1) : (m.adult ? '暂无' : '—');
  document.getElementById('dYear').textContent = m.shortDrama ? (isComicDramaMovie(m) ? '红果漫剧' : '红果短剧') : ((m.adult ? (adultReleaseDate(m) || m.y) : m.y) || '—');
  document.getElementById('dDur').textContent = (m.adult && adultDurationMin(m) ? adultDurationMin(m) + ' 分钟' : (m.d || '时长未知'));
  document.getElementById('dGenre').textContent = m.adult
    ? (adultTags(m).slice(0, 6).join(' · ') || '未分类')
    : (Array.isArray(m.genres) && m.genres.length ? m.genres.join(' · ') : (m.genre || '未分类'));
  const dDesc = document.getElementById('dDesc');
  if(dDesc) dDesc.textContent = (m.adult ? composeAdultSummary(m) : m.summary) || '暂无简介。';
  document.getElementById('dNetwork').textContent = m.adult ? (adultStudio(m) || adultLabel(m) || '未提供') : (m.network || m.sourceLabel || '未提供');
  document.getElementById('dNetworkRole').textContent = m.shortDrama ? '内容平台' : (m.adult ? '制作商' : (appMode === 'catalog' ? '播出平台' : '媒体平台'));
  document.getElementById('dStatus').textContent = m.status || (m.playUrl || m.providerId ? '可播放' : '仅元数据');
  document.getElementById('dSource').textContent = m.shortDrama
    ? (isComicDramaMovie(m) ? '红果漫剧热播榜' : '红果短剧公开目录')
    : (m.adult
    ? (javPayload(m).provider ? 'JAV · ' + String(javPayload(m).provider) : (isAdultScraped(m) ? 'JAV 刮削' : '待刮削'))
    : (appMode === 'catalog' ? (m.id ? 'TVMaze #' + m.id : '公开目录') : (m.sourceLabel || '本地媒体')));
  document.getElementById('dBadge').textContent = m.adult ? (adultCode(m) || '18+') : (m.q || String(m.type || 'VIDEO').toUpperCase());
  document.getElementById('dSourceBadge').textContent = m.adult ? `${detailSourceLabel(m)} · 18+` : detailSourceLabel(m);
  document.getElementById('dLibraryKind').textContent = m.shortDrama ? (isComicDramaMovie(m) ? '漫剧' : '短剧') : (m.adult ? '深夜档' : (appMode === 'catalog' ? '公开目录' : (m.providerId ? '已连接来源' : '本地媒体')));
  renderAdultDetailFacts(m);
  renderAdultActors(m);
  isDetailFaved = favoriteIds.has(String(m.id));
  const btn = document.getElementById('dFavBtn');
  const txt = document.getElementById('dFavText');
  if(btn && txt){ btn.style.color = isDetailFaved ? '#ff5f6b' : ''; btn.style.borderColor = isDetailFaved ? 'rgba(255,95,107,.4)' : ''; txt.textContent = isDetailFaved ? '已收藏' : '收藏'; }
  renderSimilarMovies(m);
  const detailEditButton = document.querySelector('.d-actions button[onclick*="openEditModal"]');
  const detailStatsButton = document.querySelector('.d-actions button[onclick*="openStatsModal"]');
  const detailPosterPlay = document.querySelector('#view-detail .d-poster .poster-play');
  if(detailEditButton) detailEditButton.hidden = Boolean(m.shortDrama);
  if(detailStatsButton) detailStatsButton.hidden = Boolean(m.shortDrama);
  if(detailPosterPlay){
    detailPosterPlay.onclick = m.shortDrama
      ? () => m.episodes?.[0]?.vid
        ? playEpisode(0, null, m)
        : window.open(m.sourceUrl || 'https://hongguoduanju.com/', '_blank', 'noopener,noreferrer')
      : () => openPlayer(selectedMovie, detailPosterPlay);
  }
  renderDetailVersions(m);
  const needsEpisodes = isStreamHubShow(m) && !m.episodesLoaded;
  renderDetailEpisodes(m, {loading: needsEpisodes});
  if(needsEpisodes){
    void ensureMovieEpisodes(m).then(() => {
      if(detailMovie === m){ renderDetailEpisodes(m); renderDetailVersions(m); }
    }).catch(error => {
      console.warn('Unable to load StreamHub episodes:', error);
      if(detailMovie === m) renderDetailEpisodes(m, {error: '无法读取真实剧集：' + backendErrorMessage(error)});
    });
  }

  /* 详情页内部换片只做短距离淡入，避免底部卡片跨屏拉伸到顶部海报。 */
  if(isDetailSwap){
    showView('detail');
    if(!reducedMotion){
      document.querySelectorAll('#view-detail .detail-layout, #view-detail .version-card').forEach((section, index) => {
        section.animate(
          [{opacity:.35, transform:'translateY(10px)'}, {opacity:1, transform:'translateY(0)'}],
          {duration:260, delay:index * 35, easing:'cubic-bezier(.22,1,.36,1)', fill:'both'}
        );
      });
    }
  /* 从其他页面进入详情时，保留卡片与目标海报之间的空间关联。 */
  } else if(sourceEl && !reducedMotion && !isDetailMorphing){
    isDetailMorphing = true;
    const imgEl = sourceEl.querySelector('img') || sourceEl;
    const startRect = imgEl.getBoundingClientRect();
    const startRadius = window.getComputedStyle(imgEl).borderRadius || '16px';

    if(viewDetail){
      viewDetail.classList.add('view-morphing');
      viewDetail.classList.remove('morph-done');
    }

    showView('detail');
    window.scrollTo({top: 0, behavior: 'instant'});

    const targetPoster = document.querySelector('.d-poster') || dPosterCard;
    if(targetPoster){
      targetPoster.style.opacity = '0';
    }

    const destRect = targetPoster ? targetPoster.getBoundingClientRect() : {top: 130, left: 120, width: 280, height: 420};
    const destRadius = targetPoster ? (window.getComputedStyle(targetPoster).borderRadius || '16px') : '16px';

    const morphProxy = document.createElement('div');
    morphProxy.className = 'shared-card-morph';
    morphProxy.style.top = startRect.top + 'px';
    morphProxy.style.left = startRect.left + 'px';
    morphProxy.style.width = startRect.width + 'px';
    morphProxy.style.height = startRect.height + 'px';
    morphProxy.style.borderRadius = startRadius;
    morphProxy.innerHTML = `
      <img src="${m.img}" alt="${m.t}" />
      <div class="d-src-tag"><span class="badge">${escapeHtml(m.q || String(m.type || 'VIDEO').toUpperCase())}</span><span class="badge">${escapeHtml(detailSourceLabel(m))}</span></div>
      <div class="poster-play"><span><svg viewBox="0 0 24 24" fill="#fff"><path d="M8 5.5v13l11-6.5z"></path></svg></span></div>
      <div class="shared-card-morph-shade"></div>
    `;
    document.body.appendChild(morphProxy);

    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        morphProxy.style.top = destRect.top + 'px';
        morphProxy.style.left = destRect.left + 'px';
        morphProxy.style.width = destRect.width + 'px';
        morphProxy.style.height = destRect.height + 'px';
        morphProxy.style.borderRadius = destRadius;
        morphProxy.classList.add('morphed');

        setTimeout(() => {
          if(targetPoster) targetPoster.style.opacity = '1';
          morphProxy.style.opacity = '0';
          setTimeout(() => {
            morphProxy.remove();
            isDetailMorphing = false;
          }, 80);
        }, 460);
      });
    });
  } else {
    showView('detail');
    window.scrollTo({top: 0, behavior: 'instant'});
  }
}

/* ================= 云盘挂载与扫描 ============ */
function pickSideTab(el, msg){
  el.parentElement.querySelectorAll('.cs-item').forEach(i => i.classList.remove('active'));
  el.classList.add('active');
  toast(msg);
}

let scanTimer = null;
let notificationPopoverOpen = false;
let notificationPopoverTrigger = null;
let notificationDismissHandler = null;
let notificationRepositionHandler = null;
let scanTaskSerial = 0;
const scanTasks = [];
const SCAN_TASK_STORAGE_KEY = 'ttv.notification.scanTasks.v1';
function saveScanTasks(){
  try{
    if(!scanTasks.length){
      localStorage.removeItem(SCAN_TASK_STORAGE_KEY);
      return;
    }
    const snapshot = scanTasks.slice(0, 12).map(task => ({
      ...task,
      logs: Array.isArray(task.logs) ? task.logs.slice(-80) : []
    }));
    localStorage.setItem(SCAN_TASK_STORAGE_KEY, JSON.stringify(snapshot));
  }catch(error){
    console.warn('Unable to save notification tasks:', error);
  }
}
function restoreScanTasks(){
  try{
    const raw = localStorage.getItem(SCAN_TASK_STORAGE_KEY);
    if(!raw) return;
    const parsed = JSON.parse(raw);
    if(!Array.isArray(parsed)) return;
    scanTasks.length = 0;
    for(const item of parsed.slice(0, 12)){
      if(!item || typeof item !== 'object') continue;
      const task = {
        ...createScanTask(item.kind, item.title),
        ...item,
        logs: Array.isArray(item.logs) ? item.logs.slice(-80) : []
      };
      // Older builds called the user's stop action "cancelled". Migrate it to
      // the explicit paused state so it is never resumed behind the user's back.
      if(task.status === 'cancelled' || task.status === 'paused'){
        task.active = false;
        task.status = 'paused';
        task.userPaused = true;
        task.resumeOnLaunch = false;
        task.message = task.message || '已由用户暂停。';
        task.stage = task.message;
      }
      const wasScraping = task.phase === 'scrape'
        || task.kind === 'scrape'
        || /刮削/.test(String(task.stage || task.message || ''));
      if(!task.userPaused && wasScraping && (task.active || task.status === 'interrupted' || task.status === 'error' || task.status === 'resume-pending')){
        // The desktop process ended before this scrape completed. Keep the same
        // task card and queue it for automatic recovery after startup finishes.
        task.active = false;
        task.status = 'resume-pending';
        task.finishedAt = 0;
        task.resumeOnLaunch = true;
        task.phase = 'scrape';
        task.message = '程序重新启动，正在等待自动继续刮削…';
        task.stage = task.message;
      }else if(task.active){
        task.active = false;
        task.status = 'interrupted';
        task.finishedAt = Number(task.finishedAt || Date.now());
        task.message = '程序在扫描阶段退出；已导入的数据会保留。';
        task.stage = task.message;
      }
      scanTasks.push(task);
      const serial = Number(String(task.id || '').split('-').pop());
      if(Number.isFinite(serial) && serial > scanTaskSerial) scanTaskSerial = serial;
    }
  }catch(error){
    console.warn('Unable to restore notification tasks:', error);
  }finally{
    saveScanTasks();
  }
}
function createScanTask(kind, title){
  return {
    id: `scan-${Date.now()}-${++scanTaskSerial}`,
    kind: kind || 'scan', title: title || '资源扫描与刮削',
    active: true, status: 'running', stage: '准备开始', percent: null,
    phase: 'prepare', userPaused: false, resumeOnLaunch: false,
    scrapeOptions: null, retryCount: 0,
    startedAt: Date.now(), finishedAt: 0, message: '', logs: [],
    total:0, folders:0, files:0, imported:0, updated:0, skipped:0,
    promotional:0, nonVideo:0, matched:0, covers:0, adultIsolated:0
  };
}
let scanProgress = createScanTask('scan', '资源扫描与刮削');
scanProgress.active = false;
scanProgress.status = 'idle';
function notificationTaskState(task){
  if(task.active) return {label:'进行中', cls:'is-running'};
  if(task.status === 'resume-pending') return {label:'自动恢复中', cls:'is-running'};
  if(task.status === 'paused') return {label:'已暂停', cls:'is-paused'};
  if(task.status === 'error') return {label:'失败', cls:'is-error'};
  if(task.status === 'interrupted') return {label:'已中断', cls:'is-interrupted'};
  return {label:'已完成', cls:'is-done'};
}
// 小于 10% 时显示一位小数，否则大批量早期永远显示 0%。
function fmtPercent(value){
  const n = Math.max(0, Math.min(100, Number(value) || 0));
  return n > 0 && n < 10 ? n.toFixed(1) : String(Math.round(n));
}
function renderNotificationTasks(){
  const list = document.getElementById('notificationTaskList');
  const runningCount = scanTasks.filter(task => task.active || task.status === 'resume-pending').length;
  document.querySelectorAll('.notification-badge').forEach(badge => {
    badge.textContent = runningCount > 9 ? '9+' : String(runningCount || '');
    badge.classList.toggle('visible', runningCount > 0);
  });
  if(!list) return;
  if(!scanTasks.length){
    list.innerHTML = '<div class="notification-empty">当前没有扫描或刮削任务。</div>';
    return;
  }
  list.innerHTML = scanTasks.map(task => {
    const percent = Number.isFinite(task.percent) ? Math.max(0, Math.min(100, Math.round(task.percent))) : null;
    const state = notificationTaskState(task);
    // 「已处理」是本轮跑过的条目数(含未命中,只写了阶段/隔离标记),
    // 「命中」才是真正刮到元数据的条目数 —— 与深夜档「已刮削」同口径。
    const counts = `发现 ${task.files || 0} · 入库 ${task.imported || 0} · 命中 ${task.matched || 0} · 已处理 ${task.updated || 0}`;
    const running = task.active || task.status === 'resume-pending';
    const bar = running && percent === null
      ? '<div class="notification-task-bar indeterminate"><i></i></div>'
      : `<div class="notification-task-bar"><i style="width:${percent || 0}%"></i></div>`;
    const latest = task.message || task.stage || (running ? '处理中' : '');
    const resumable = task.status === 'paused' || task.status === 'error' || task.status === 'interrupted' || task.status === 'resume-pending';
    // 已入库过内容的中断任务可以「继续刮削」（只补刮未刮的记录，不重爬目录）；
    // 纯刮削任务沿用原「继续」。
    const canContinue = resumable && (task.kind === 'scrape' || Number(task.imported || 0) > 0);
    const continueLabel = task.kind === 'scrape' ? '继续' : '继续刮削';
    const actions = running
      ? `<button class="notification-task-btn" onclick="pauseScanTask('${task.id}')">暂停</button>`
      : `<button class="notification-task-btn" onclick="removeScanTask('${task.id}')">删除</button>`
        + (canContinue
          ? `<button class="notification-task-btn is-continue" onclick="continueScanTask('${task.id}')">${continueLabel}</button>`
          : '')
        + (resumable && task.status !== 'paused' && task.status !== 'resume-pending'
          ? `<button class="notification-task-btn" onclick="retryScanTask('${task.id}')">重试</button>`
          : '');
    return `<article class="notification-task ${state.cls}" data-task-id="${task.id}">
      <div class="notification-task-head"><strong>${escapeHtml(task.title)}</strong><span>${state.label}</span></div>
      ${bar}
      <div class="notification-task-meta"><span>${escapeHtml(latest)}</span><b>${running ? (percent === null ? '处理中' : fmtPercent(percent) + '%') : (percent === null ? '—' : fmtPercent(percent) + '%')}</b></div>
      <div class="notification-task-counts">${escapeHtml(counts)}</div>
      <div class="notification-task-actions">${actions}</div>
    </article>`;
  }).join('');
}
function positionNotificationPopover(trigger = notificationPopoverTrigger){
  const popover = document.getElementById('notificationPopover');
  if(!popover || !trigger) return;
  const sourceRect = trigger.getBoundingClientRect();
  const rect = sourceRect;
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  const gap = 10;
  const width = Math.min(360, viewportWidth - 24);
  const anchorOnRight = rect.left + rect.width / 2 >= viewportWidth / 2;
  popover.style.width = `${width}px`;
  popover.style.left = '';
  popover.style.right = '';
  if(anchorOnRight){
    popover.style.right = `${Math.max(12, viewportWidth - rect.right)}px`;
  }else{
    const left = Math.max(12, Math.min(viewportWidth - width - 12, rect.left + rect.width / 2 - width / 2));
    popover.style.left = `${left}px`;
  }
  popover.style.top = `${rect.bottom + gap}px`;
  popover.classList.toggle('opens-upward', rect.bottom + gap + popover.offsetHeight > viewportHeight - 12);
  if(popover.classList.contains('opens-upward')){
    popover.style.top = `${Math.max(12, rect.top - popover.offsetHeight - gap)}px`;
  }
}
function closeNotificationPopover(){
  const popover = document.getElementById('notificationPopover');
  if(!popover || !notificationPopoverOpen) return;
  notificationPopoverOpen = false;
  popover.classList.remove('open', 'opens-upward');
  popover.setAttribute('aria-hidden', 'true');
  notificationPopoverTrigger?.setAttribute('aria-expanded', 'false');
  notificationPopoverTrigger = null;
  if(notificationDismissHandler) document.removeEventListener('pointerdown', notificationDismissHandler, true);
  if(notificationRepositionHandler) window.removeEventListener('resize', notificationRepositionHandler);
  notificationDismissHandler = null;
  notificationRepositionHandler = null;
}
function toggleNotificationPopover(event){
  event?.preventDefault();
  event?.stopPropagation();
  const trigger = event?.currentTarget || document.querySelector('.notification-trigger');
  if(notificationPopoverOpen && notificationPopoverTrigger === trigger){
    closeNotificationPopover();
    return;
  }
  if(notificationPopoverOpen) closeNotificationPopover();
  const popover = document.getElementById('notificationPopover');
  if(!popover || !trigger) return;
  notificationPopoverOpen = true;
  notificationPopoverTrigger = trigger;
  trigger.setAttribute('aria-expanded', 'true');
  popover.classList.add('open');
  popover.setAttribute('aria-hidden', 'false');
  renderNotificationTasks();
  positionNotificationPopover(trigger);
  if(TtvBackend.available()){
    TtvBackend.invoke('runtime_status').then(runtime => {
      const runtimeEl = document.getElementById('notificationRuntime');
      if(runtimeEl) runtimeEl.textContent = runtime?.playbackAvailable === false ? '桌面播放运行时不可用' : '任务状态会实时更新';
    }).catch(error => {
      const runtimeEl = document.getElementById('notificationRuntime');
      if(runtimeEl) runtimeEl.textContent = '无法读取桌面运行时状态';
      console.warn('Unable to read notification runtime:', error);
    });
  }
  notificationDismissHandler = click => {
    if(!popover.contains(click.target) && !click.target.closest('.notification-trigger')) closeNotificationPopover();
  };
  notificationRepositionHandler = () => positionNotificationPopover();
  document.addEventListener('pointerdown', notificationDismissHandler, true);
  window.addEventListener('resize', notificationRepositionHandler);
}
function pauseScanTask(taskId){
  const task = scanTasks.find(item => item.id === taskId);
  if(!task || (!task.active && task.status !== 'resume-pending')) return;
  const markPaused = () => {
    task.active = false;
    task.status = 'paused';
    task.userPaused = true;
    task.resumeOnLaunch = false;
    task.finishedAt = Date.now();
    task.message = '已由用户暂停。已导入和已刮削的数据都会保留。';
    task.stage = task.message;
    if(scanProgress.id === task.id) scanProgress = task;
    renderNotificationTasks();
    saveScanTasks();
    toast('刮削任务已暂停。');
  };
  // Persist the user's intent before asking the backend to stop. If the app is
  // closed during this IPC call, startup still knows it must not auto-resume.
  markPaused();
  if(!TtvBackend.available()){
    return;
  }
  TtvBackend.invoke('tasks_cancel').then(ok => {
    if(!ok) throw new Error('cancel rejected');
  }).catch(error => {
    task.message = '暂停请求暂未送达，将保持暂停并在下次启动时继续尊重此状态：' + backendErrorMessage(error);
    task.stage = task.message;
    renderNotificationTasks();
    saveScanTasks();
    toast('暂停请求暂未送达，但任务不会被自动恢复。');
  });
}
// Compatibility for any stale inline handler left in an already-open WebView.
function cancelScanTask(taskId){ pauseScanTask(taskId); }
function removeScanTask(taskId){
  const index = scanTasks.findIndex(item => item.id === taskId);
  if(index < 0) return;
  scanTasks.splice(index, 1);
  if(scanProgress.id === taskId){
    scanProgress = createScanTask(scanProgress.kind, scanProgress.title);
    scanProgress.active = false;
    scanProgress.status = 'idle';
  }
  renderNotificationTasks();
  saveScanTasks();
}
function clearFinishedScanTasks(){
  for(let i = scanTasks.length - 1; i >= 0; i--){
    const task = scanTasks[i];
    if(!task.active && task.status !== 'paused' && task.status !== 'error' && task.status !== 'interrupted' && task.status !== 'resume-pending') scanTasks.splice(i, 1);
  }
  renderNotificationTasks();
  saveScanTasks();
}
// Restart a failed/paused/interrupted task. Cloud tasks remember their
// folder selection + provider, so 重试 re-runs the exact same scan without
// re-picking folders; the import uses upserts, so already-imported items are
// only refreshed, never duplicated.
function retryScanTask(taskId){
  const task = scanTasks.find(item => item.id === taskId);
  if(!task || task.active) return;
  removeScanTask(taskId);
  if(task.kind === 'scrape') scrapeCurrentLibrary();
  else if(task.kind === 'local') startScanPipeline();
  else if(task.kind === 'cloud' || task.kind === 'openlist'){
    const folders = Array.isArray(task.selectedFolders)
      ? task.selectedFolders.map(folder => [folder.id, folder.name])
      : [];
    const source = (SOURCE_CATALOG.find(item => item.id === task.providerId))
      || {id: task.providerId || 'guangya', name: '云盘'};
    if(folders.length && task.providerId){
      runCloudFoldersScanTask(source, folders, null);
    }else{
      toast('该任务未记录目录清单，请在对应的云盘目录重新点击「扫描并入库」。');
    }
  }
  else toast('请重新发起扫描。');
}
// Resume an interrupted/failed/paused task. Unlike 重试 (a fresh full run),
// 继续 keeps the SAME task card and only processes the unscraped backlog
// (overwrite=false) — for a cloud task interrupted mid-import this skips the
// slow directory re-walk entirely and scrapes what is already in the library.
// The interrupted leg's counters are frozen as continueBase: live progress
// adds on top of them instead of dropping back to 0.
async function continueScanTask(taskId){
  const task = scanTasks.find(item => item.id === taskId);
  if(!task || task.active) return;
  const canResumeScrape = task.phase === 'scrape' || task.resumeOnLaunch || task.kind === 'scrape'
    || ((task.kind === 'cloud' || task.kind === 'openlist' || task.kind === 'local') && Number(task.imported || 0) > 0);
  if(!canResumeScrape){ retryScanTask(taskId); return; }
  if(!TtvBackend.available()){ toast('继续任务只能在桌面端执行。'); return; }
  task.active = true;
  task.status = 'running';
  task.userPaused = false;
  task.resumeOnLaunch = true;
  task.phase = 'scrape';
  task.finishedAt = 0;
  task.message = '继续刮削未完成的记录…';
  task.stage = task.message;
  task.continueBase = {
    files: Number(task.files || 0),
    matched: Number(task.matched || 0),
    updated: Number(task.updated || 0),
    skipped: Number(task.skipped || 0),
    covers: Number(task.covers || 0),
    adultIsolated: Number(task.adultIsolated || 0),
    percent: Number.isFinite(task.percent) ? Math.max(0, Math.min(100, Number(task.percent))) : 0
  };
  scanProgress = task;
  logScanProgress('继续刮削：只处理尚未刮削的记录，已完成的不会重复。');
  renderNotificationTasks();
  saveScanTasks();
  try{
    const report = await scrapeLibraryUntilDone(5000);
    const base = task.continueBase || {};
    task.matched = (base.matched || 0) + Number(report?.matched || 0);
    task.updated = (base.updated || 0) + Number(report?.updated || 0);
    task.skipped = (base.skipped || 0) + Number(report?.unmatched || 0);
    task.covers = (base.covers || 0) + Number(report?.covers || 0);
    task.adultIsolated = (base.adultIsolated || 0) + Number(report?.adultIsolated || 0);
    if(base.files) task.files = base.files;
    task.continueBase = null;
    // Pausing breaks the backend loop and still resolves with a partial report;
    // keep the paused state instead of overwriting it with done.
    if(!task.active || task.status === 'paused'){
      renderNotificationTasks();
      saveScanTasks();
      return;
    }
    updateScanProgress('刮削完成，刷新影视库', 100);
    await refreshLibraryAfterImport();
    const resumeHint = task.kind !== 'scrape' && Array.isArray(task.selectedFolders) && task.selectedFolders.length
      ? '如还有目录未导入，点「重试」会按原清单重新扫描。'
      : '';
    finishScanProgress(`继续刮削完成：本次补刮 ${report?.updated || 0} 条，累计刮削 ${task.updated} 条。${resumeHint}`);
    toast('继续刮削完成。' + scrapeSummary(report));
  }catch(error){
    // Keep whatever the live listener accumulated so 继续 can be pressed again.
    task.continueBase = null;
    finishScanProgress(backendErrorMessage(error), true);
    toast('继续失败：' + backendErrorMessage(error));
  }
}
let autoResumeScrapeInFlight = false;
async function resumePersistedScrapeTasks(){
  if(autoResumeScrapeInFlight || scanProgress.active) return;
  const task = scanTasks.find(item => item.status === 'resume-pending' && item.resumeOnLaunch && !item.userPaused);
  if(!task) return;
  if(!TtvBackend.available()){
    window.setTimeout(resumePersistedScrapeTasks, 5000);
    return;
  }
  autoResumeScrapeInFlight = true;
  task.message = '程序已重新启动，正在自动继续未完成的刮削任务…';
  task.stage = task.message;
  renderNotificationTasks();
  saveScanTasks();
  try{
    await continueScanTask(task.id);
  }finally{
    autoResumeScrapeInFlight = false;
    if(scanTasks.some(item => item.status === 'resume-pending' && item.resumeOnLaunch && !item.userPaused)){
      window.setTimeout(resumePersistedScrapeTasks, 1000);
    }
  }
}
function resetScanProgress(kind, title){
  const pendingResume = scanTasks.find(task => task.status === 'resume-pending' && task.resumeOnLaunch && !task.userPaused);
  if(scanProgress.active || pendingResume){
    renderNotificationTasks();
    toast('已有扫描或刮削任务正在运行或等待自动恢复，请等待完成或先在通知中心暂停。');
    return false;
  }
  for(const key of Object.keys(scanProgressTaskTotals)) delete scanProgressTaskTotals[key];
  scanProgress = createScanTask(kind, title);
  scanTasks.unshift(scanProgress);
  if(scanTasks.length > 12) scanTasks.length = 12;
  saveScanTasks();
  const titleEl = document.getElementById('scanProgressTitle');
  const subtitle = document.getElementById('scanProgressSubtitle');
  const state = document.getElementById('scanProgressState');
  const log = document.getElementById('scanProgressLog');
  if(titleEl) titleEl.textContent = title || '资源扫描与刮削';
  if(subtitle) subtitle.textContent = '任务正在执行，视频会先导入影视库，再使用已启用平台补全海报、简介和年份。';
  if(state){ state.textContent = '进行中'; state.className = 'scan-progress-state running'; }
  if(log) log.innerHTML = '';
  updateScanProgress('准备开始', null);
  renderNotificationTasks();
  // 无论用户停留在哪个页面，通知按钮都会立刻出现进行中的角标；打开面板即可看到本轮反馈。
  toast('扫描任务已开始，通知中心会实时显示进度。');
  return true;
}
function logScanProgress(message){
  const text = String(message || '');
  scanProgress.logs.push({time: Date.now(), message: text});
  scanProgress.message = text;
  saveScanTasks();
  const log = document.getElementById('scanProgressLog');
  if(!log){ renderNotificationTasks(); return; }
  const row = document.createElement('div');
  row.className = 'scan-progress-log-line';
  row.innerHTML = `<time>${new Date().toLocaleTimeString()}</time><span>${escapeHtml(text)}</span>`;
  log.appendChild(row);
  log.scrollTop = log.scrollHeight;
  renderNotificationTasks();
  saveScanTasks();
}
function updateScanProgress(stage, percent){
  scanProgress.stage = stage || '处理中';
  scanProgress.message = scanProgress.stage;
  scanProgress.percent = Number.isFinite(percent) ? Math.max(0, Math.min(100, Number(percent))) : null;
  const fill = document.getElementById('scanProgressFill');
  const track = document.getElementById('scanProgressTrack');
  const stageEl = document.getElementById('scanProgressStage');
  const percentEl = document.getElementById('scanProgressPercent');
  const determinate = Number.isFinite(percent);
  if(stageEl) stageEl.textContent = stage || '处理中';
  if(track) track.classList.toggle('indeterminate', !determinate);
  if(percentEl) percentEl.textContent = determinate ? `${fmtPercent(percent)}%` : '处理中';
  if(fill){
    // Clear the inline width when switching back to the indeterminate
    // animation, otherwise the leftover width fights the keyframe transform
    // and the bar appears stuck part-way.
    if(determinate) fill.style.width = `${Math.max(0, Math.min(100, percent))}%`;
    else fill.style.width = '';
  }
  const current = document.getElementById('scanProgressCurrent');
  if(current) current.textContent = stage || '处理中';
  for(const [id, value] of [['scanProgressFolders',scanProgress.folders],['scanProgressFiles',scanProgress.files],['scanProgressImported',scanProgress.imported],['scanProgressUpdated',scanProgress.updated],['scanProgressSkipped',scanProgress.skipped],['scanProgressPromotional',scanProgress.promotional],['scanProgressNonVideo',scanProgress.nonVideo],['scanProgressMatched',scanProgress.matched],['scanProgressCovers',scanProgress.covers],['scanProgressAdultIsolated',scanProgress.adultIsolated]]){
    const el = document.getElementById(id);
    if(el) el.textContent = String(value || 0);
  }
  renderNotificationTasks();
  saveScanTasks();
}
// Live import progress. `provider_sync_library_recursive` emits
// `library://scan-progress` per directory page while it walks the cloud; we
// aggregate the ticks per task so concurrent scans add up in one card instead
// of the counters sitting at zero for the whole import.
const scanProgressTaskTotals = {};
function setupScanProgressListener(){
  const listen = window.__TAURI__?.event?.listen;
  if(typeof listen !== 'function') return;
  listen('library://scan-progress', (event) => {
    const p = event && event.payload;
    if(!p || !scanProgress.active || scanProgress.kind === 'scrape') return;
    scanProgressTaskTotals[String(p.taskKey || 'default')] = p;
    const totals = {folders:0, files:0, imported:0, skipped:0, promotional:0, nonVideo:0};
    for(const tick of Object.values(scanProgressTaskTotals)){
      totals.folders += Number(tick.folders || 0);
      totals.files += Number(tick.fetched || 0);
      totals.imported += Number(tick.imported || 0);
      totals.skipped += Number(tick.skipped || 0);
      totals.promotional += Number(tick.skippedPromotional || 0);
      totals.nonVideo += Number(tick.skippedNonVideo || 0);
    }
    scanProgress.folders = totals.folders;
    scanProgress.files = totals.files;
    scanProgress.imported = totals.imported;
    scanProgress.skipped = totals.skipped;
    scanProgress.promotional = totals.promotional;
    scanProgress.nonVideo = totals.nonVideo;
    const folder = p.currentFolder ? `：${p.currentFolder}` : '';
    updateScanProgress(`正在扫描${folder} · 已读取 ${totals.files} 项 · 入库 ${totals.imported}`, null);
  }).catch(() => {});
}
// Live per-item scrape progress. The backend emits `library://scrape-progress`
// for every item it processes; we mirror those counters into the scan-progress
// view so the user sees match/cover/isolation counts move in real time instead
// of a single blocking spinner.
// 刮削期间把新命中的元数据节流同步进内存库(深夜档统计/网格随之刷新):
// 运行中每 60 秒至多一次,批次结束时强制一次。
let lastCatalogSyncAt = 0;
async function syncLibraryThrottled(force = false){
  const now = Date.now();
  if(!force && now - lastCatalogSyncAt < 60 * 1000) return;
  lastCatalogSyncAt = now;
  try{ await refreshLibraryAfterImport(); }catch(error){ /* 后台同步失败不打断刮削 */ }
}
function setupScrapeProgressListener(){
  const listen = window.__TAURI__?.event?.listen;
  if(typeof listen !== 'function') return;
  listen('library://scrape-progress', (event) => {
    const p = event && event.payload;
    if(!p) return;
    // A late backend tick can arrive just after the user pressed pause. Do not
    // create a new task card or revive the paused task from that stale event.
    if(!scanProgress.active && scanTaskPausedByUser()) return;
    if(!scanProgress.active){
      const resumed = scanTasks.find(task => !task.userPaused
        && (task.kind === 'scrape' || task.phase === 'scrape')
        && (task.status === 'resume-pending' || task.status === 'interrupted' || task.status === 'error'));
      if(resumed){
        resumed.active = true;
        resumed.status = 'running';
        resumed.recoveredFromEvent = true;
        resumed.message = '已接回刮削进度。';
        scanProgress = resumed;
      }else{
        resetScanProgress('scrape', '资源刮削进度');
      }
    }
    // A continued task freezes its pre-interrupt counters in continueBase;
    // live per-run counters are added on top so the card keeps accumulating
    // instead of resetting, and the percent resumes from the breakpoint.
    const base = scanProgress.continueBase || null;
    scanProgress.matched = (base?.matched || 0) + Number(p.matched || 0);
    scanProgress.updated = (base?.updated || 0) + Number(p.updated || 0);
    scanProgress.covers = (base?.covers || 0) + Number(p.covers || 0);
    scanProgress.adultIsolated = (base?.adultIsolated || 0) + Number(p.adultIsolated || 0);
    // Only overwrite the "files / filtered" cells for a pure scrape; during a
    // local scan those cells already carry the import-phase counts.
    if(scanProgress.kind === 'scrape'){
      scanProgress.files = Number(base?.files || p.total || 0);
      scanProgress.skipped = (base?.skipped || 0) + Number(p.unmatched || 0);
    }
    const runPercent = Number(p.percent || 0);
    const percent = base
      ? (base.percent || 0) + (100 - (base.percent || 0)) * runPercent / 100
      : runPercent;
    if(p.done){
      updateScanProgress('刮削完成，正在写回影视库', 100);
      logScanProgress(`刮削完成：匹配 ${scanProgress.matched} 条，未匹配 ${scanProgress.skipped} 条，缓存封面 ${scanProgress.covers} 张，隔离 18+ ${scanProgress.adultIsolated} 条。`);
      syncLibraryThrottled(true);
      if(scanProgress.recoveredFromEvent){
        scanProgress.recoveredFromEvent = false;
        finishScanProgress('自动接回的刮削任务已完成。');
      }
    }else{
      const label = p.title ? `：${p.title}` : '';
      const provider = p.provider ? `（${p.provider}）` : '';
      updateScanProgress(`正在刮削 ${p.current}/${p.total}${provider}${label}`, percent);
      syncLibraryThrottled(false);
    }
  }).catch(() => {});
}
function finishScanProgress(message, error = false){
  scanProgress.active = false;
  scanProgress.status = error ? 'error' : 'done';
  if(!error){
    scanProgress.phase = 'done';
    scanProgress.resumeOnLaunch = false;
    scanProgress.retryCount = 0;
  }
  scanProgress.finishedAt = Date.now();
  // A finished task genuinely reached the end; failed tasks keep whatever
  // progress they had so the card does not lie about how far it got.
  if(!error) scanProgress.percent = 100;
  updateScanProgress(error ? '任务失败' : '任务完成', error ? (Number.isFinite(scanProgress.percent) ? scanProgress.percent : null) : 100);
  const state = document.getElementById('scanProgressState');
  if(state){ state.textContent = error ? '失败' : '已完成'; state.className = `scan-progress-state ${error ? 'error' : 'done'}`; }
  const button = document.getElementById('scanProgressLibrary');
  if(button) button.disabled = false;
  if(message) logScanProgress(message);
  renderNotificationTasks();
  saveScanTasks();
}
async function refreshLibraryAfterImport(){
  await loadInitialCatalog();
  const button = document.getElementById('scanProgressLibrary');
  if(button) button.disabled = false;
}
function startScanPipeline(){
  if(!TtvBackend.available()){
    toast('本地目录扫描仅可在 TTV 桌面端使用。');
    return;
  }
  openModal(
    '扫描本地媒体目录',
    `
      <div class="modal-field">
        <label>媒体目录</label>
        <input class="modal-input" id="scanRoot" placeholder="例如 D:\\Media 或 E:\\Movies" autocomplete="off" />
      </div>
      <div class="modal-field">
        <label>最多扫描文件数</label>
        <input class="modal-input" id="scanMaxFiles" type="number" min="1" max="1000000" value="5000" />
      </div>
      <div class="modal-field">
        <label>18+ (NSFW) 标记</label>
        <select class="modal-input" id="scanMarkAdult">
          <option value="auto" selected>自动判断（按文件名识别）</option>
          <option value="adult">整批标记为 18+（NSFW）</option>
          <option value="normal">不强行标为 18+（番号命中仍会隔离）</option>
        </select>
      </div>
    `,
    `
      <button class="btn btn-ghost" onclick="closeModal()">取消</button>
      <button class="btn btn-accent" onclick="scanLocalDirectory()">开始扫描</button>
    `
  );
}
async function scanLocalDirectory(){
  const pipeNum = document.getElementById('pipeNum');
  const pipeStatus = document.getElementById('pipeStatusText');
  const pipeDir = document.getElementById('pipeDirCount');
  const pipeVid = document.getElementById('pipeVidCount');
  const pipeLst = document.getElementById('pipeLstCount');
  const cloudScanStatus = document.getElementById('cloudScanStatus');
  const root = document.getElementById('scanRoot')?.value.trim();
  const maxFiles = Number(document.getElementById('scanMaxFiles')?.value || 5000);
  const markAdultChoice = document.getElementById('scanMarkAdult')?.value || 'auto';
  const markAdult = markAdultChoice === 'adult' ? true : (markAdultChoice === 'normal' ? false : null);
  if(!root){ toast('请输入要扫描的本地目录。'); return; }
  closeModal();
  if(!resetScanProgress('local', '扫描本地媒体目录')) return;
  logScanProgress(`开始扫描：${root}`);
  pipeStatus.textContent = '正在扫描本地目录...';
  pipeStatus.style.color = 'var(--accent)';
  pipeNum.textContent = '…';
  try{
    updateScanProgress('正在遍历目录并写入影视库', null);
    const report = await TtvBackend.invoke('library_scan', {root, maxFiles: Math.max(1, Math.min(1000000, maxFiles)), ...(markAdult === null ? {} : {markAdult})});
    scanProgress.files = Number(report.scannedFiles || 0);
    scanProgress.imported = Number(report.imported || 0);
    scanProgress.skipped = Number(report.skipped || 0);
    scanProgress.promotional = Number(report.skippedPromotional || 0);
    scanProgress.nonVideo = Number(report.skippedNonVideo || 0);
    updateScanProgress('目录扫描完成，开始刮削元数据', null);
    logScanProgress(`发现 ${scanProgress.files} 个文件，导入 ${scanProgress.imported} 个视频，过滤 ${scanProgress.skipped} 项（广告/推广 ${scanProgress.promotional}，非视频 ${scanProgress.nonVideo}）`);
    pipeNum.textContent = '100%';
    pipeDir.textContent = report.scannedFiles || 0;
    pipeVid.textContent = report.imported || 0;
    pipeLst.textContent = report.skipped || 0;
    pipeStatus.textContent = '扫描完成 · 已导入 ' + (report.imported || 0) + ' 条';
    pipeStatus.style.color = '#10b981';
    if(cloudScanStatus) cloudScanStatus.textContent = '本地扫描已导入 ' + (report.imported || 0) + ' 条';
    const scraped = await scrapeLibraryUntilDone(5000);
    if(scanTaskPausedByUser()) return;
    scanProgress.updated = Number(scraped?.updated || 0);
    updateScanProgress('刮削完成，刷新影视库', null);
    await refreshLibraryAfterImport();
    finishScanProgress(`影视库已刷新，共 ${scanProgress.imported} 个视频，刮削 ${scanProgress.updated} 条。`);
    toast('扫描完成：导入 ' + (report.imported || 0) + ' 条，跳过 ' + (report.skipped || 0) + ' 条。' + scrapeSummary(scraped));
  }catch(error){
    pipeNum.textContent = '失败';
    pipeStatus.textContent = '扫描失败 · ' + backendErrorMessage(error);
    pipeStatus.style.color = '#ff5f6b';
    finishScanProgress(backendErrorMessage(error), true);
    toast('扫描失败：' + backendErrorMessage(error));
  }
}

function scrapeSummary(report){
  if(!report) return '';
  return report.updated ? ' 已刮削 ' + report.updated + ' 条元数据。' : '';
}
function scanTaskPausedByUser(task = scanProgress){
  return Boolean(task && (task.userPaused || task.status === 'paused'));
}
function persistentScrapeRetryDelay(attempt){
  const seconds = [5, 10, 20, 30, 60][Math.min(4, Math.max(0, attempt - 1))];
  return seconds * 1000;
}
async function waitForPersistentScrapeRetry(delayMs, task){
  const until = Date.now() + delayMs;
  while(Date.now() < until){
    if(scanTaskPausedByUser(task) || !task.active) return false;
    await new Promise(resolve => setTimeout(resolve, Math.min(1000, Math.max(0, until - Date.now()))));
  }
  return !scanTaskPausedByUser(task) && task.active;
}
async function invokePersistentLibraryScrape(input){
  const task = scanProgress;
  // Non-task callers retain normal error behavior. Every user-visible scrape
  // task, however, survives temporary network/provider/backend failures.
  if(!task?.active) return TtvBackend.invoke('library_scrape', {input});
  let attempt = Number(task.retryCount || 0);
  while(task.active && !scanTaskPausedByUser(task)){
    try{
      const report = await TtvBackend.invoke('library_scrape', {input});
      task.retryCount = 0;
      saveScanTasks();
      return report;
    }catch(error){
      if(scanTaskPausedByUser(task) || !task.active) return null;
      attempt += 1;
      task.retryCount = attempt;
      const delayMs = persistentScrapeRetryDelay(attempt);
      const delaySeconds = Math.round(delayMs / 1000);
      const reason = backendErrorMessage(error);
      updateScanProgress(`刮削源暂时不可用，${delaySeconds} 秒后自动重试（第 ${attempt} 次）`, null);
      logScanProgress(`刮削未停止：${reason}。将在 ${delaySeconds} 秒后自动重试。`);
      if(!await waitForPersistentScrapeRetry(delayMs, task)) return null;
    }
  }
  return null;
}
function scanFilterSummary(){
  const parts = [];
  if(scanProgress.promotional) parts.push(`广告/推广 ${scanProgress.promotional}`);
  if(scanProgress.nonVideo) parts.push(`非视频 ${scanProgress.nonVideo}`);
  return parts.length ? `（${parts.join('，')}）` : '';
}

async function scrapeLibraryMedia(limit = 100, overwrite = false, javScope = 'fast'){
  if(!TtvBackend.available()) return null;
  return invokePersistentLibraryScrape({limit:Math.max(1, Math.min(5000, Number(limit) || 100)), includeAdult:true, overwrite, ...(javScope === 'fast' ? {javScope:'fast'} : {})});
}
// One `library_scrape` invoke processes at most 5000 items (backend clamp),
// so a library bigger than that would silently leave most of it unscraped
// after a scan. Loop passes until the unscraped backlog is drained (a pass
// that returns fewer items than the limit means there is nothing left), and
// stop early when a pass matched nothing — the remaining backlog is then the
// slow-sources' job, not this scope's. `javScope` drives the two-phase adult
// strategy: 'fast' = JavBus only; 'full' = all six sources for the leftovers.
// Returns the SUM across passes so the finish message reflects everything.
async function scrapeLibraryUntilDone(passLimit = 5000, maxPasses = Number.POSITIVE_INFINITY, javScope = 'full', overwrite = false){
  const task = scanProgress;
  const persistentTask = task?.active ? task : null;
  if(persistentTask){
    task.phase = 'scrape';
    task.resumeOnLaunch = true;
    task.userPaused = false;
    task.scrapeOptions = {
      passLimit: Math.max(1, Math.min(5000, Number(passLimit) || 5000)),
      maxPasses: Number.isFinite(maxPasses) ? Math.max(1, Number(maxPasses)) : 0,
      javScope,
      overwrite: Boolean(overwrite)
    };
    saveScanTasks();
  }
  let total = {updated:0, matched:0, unmatched:0, covers:0, requested:0, adultIsolated:0};
  let last = null;
  for(let pass = 1; pass <= maxPasses; pass++){
    // 用户取消后不能再开下一批：library_scrape 每次调用会重置共享取消标志，
    // 若在这里继续 invoke，等于把用户刚点的取消清掉、又刮几个小时。
    if(persistentTask && (scanTaskPausedByUser(persistentTask) || !persistentTask.active)) break;
    if(scanProgress.active){
      const scopeLabel = javScope === 'fast' ? '快速源（豆瓣/JavBus）' : '全部源';
      updateScanProgress(pass === 1 ? `开始刮削元数据 · ${scopeLabel}` : `刮削第 ${pass} 批 · ${scopeLabel}（每批上限 ${passLimit} 条）`, null);
    }
    last = await scrapeLibraryMedia(passLimit, overwrite, javScope);
    if(!last) break;
    total.updated += Number(last.updated || 0);
    total.matched += Number(last.matched || 0);
    total.unmatched += Number(last.unmatched || 0);
    total.covers += Number(last.covers || 0);
    total.adultIsolated += Number(last.adultIsolated || 0);
    total.requested += Number(last.requested || 0);
    if(scanProgress.active){
      logScanProgress(`刮削第 ${pass} 批（${javScope === 'fast' ? '快速源' : '全部源'}）完成：处理 ${last.requested || 0} 条，命中 ${last.matched || 0} 条，封面 ${last.covers || 0} 张。`);
    }
    if(Number(last.requested || 0) < passLimit) break;
    // 整批零命中说明剩下的都是本scope刮不到的条目，换下一个阶段而不是空转。
    if(Number(last.updated || 0) === 0 && Number(last.matched || 0) === 0) break;
  }
  return {...last, ...total, requested: total.requested};
}
// After a scrape that processed candidates but matched nothing, check whether the
// real cause is a missing TMDB key. Movies / anime movies have no other provider
// (TVMaze only covers series), so without TMDB they can never match and the
// library keeps showing raw filenames. Surface an actionable warning in that case
// instead of leaving the user guessing. Returns true when the warning fired.
async function maybeWarnTmdbMissing(report){
  try{
    if(!report) return false;
    const requested = Number(report.requested || 0);
    const matched = Number(report.matched || 0);
    if(!requested || matched > 0) return false;
    if(!TtvBackend.available()) return false;
    const status = await TtvBackend.invoke('metadata_tmdb_status');
    if(status && status.configured) return false;
    openModal(
      '刮削完成，但未匹配到元数据',
      `
        <div style="display:flex;flex-direction:column;gap:10px">
          <p style="color:var(--text-dim);line-height:1.7;font-size:13px;margin:0">本次刮削处理了 ${requested} 条记录，但没有匹配到任何元数据，影视库仍会显示原始文件名。</p>
          <p style="color:var(--text-dim);line-height:1.7;font-size:13px;margin:0">原因：尚未配置 TMDB 密钥。电影与动漫电影必须通过 TMDB 刮削，而 TVMaze 只覆盖剧集、无法识别电影。</p>
          <p style="color:var(--text-faint);line-height:1.6;font-size:12px;margin:0">前往 themoviedb.org 的 Settings → API 免费获取 API Key，点击下方按钮配置后重新刮削，即可补全海报、简介与年份。密钥只保存在本机，不会上传。</p>
        </div>
      `,
      `
        <button class="btn btn-ghost" onclick="closeModal()">稍后再说</button>
        <button class="btn btn-accent" onclick="configureTmdbToken()">去配置 TMDB 密钥</button>
      `
    );
    return true;
  }catch(error){
    return false;
  }
}

async function scrapeCurrentLibrary(){
  if(!resetScanProgress('scrape', '刮削影视库元数据')) return;
  logScanProgress('开始处理已导入的影视库记录。注意：单独刮削不会扫描云盘；请先在云盘文件管理器中添加文件夹。');
  try{
    const providers = await TtvBackend.invoke('metadata_providers').catch(() => []);
    const enabled = Array.isArray(providers) ? providers.filter(item => item.enabled).map(item => item.name).join(' + ') : '已启用平台';
    updateScanProgress(`使用 ${enabled} 匹配媒体`, null);
    // overwrite=true 会重选全部条目（不是只选未刮削的），循环只会反复处理同
    // 一批前 5000 条——所以这里保持单遍，只升级为全量源。
    const report = await scrapeLibraryUntilDone(5000, 1, 'full', true);
    if(scanTaskPausedByUser()) return;
    scanProgress.files = Number(report?.requested || 0);
    scanProgress.matched = Number(report?.matched || 0);
    scanProgress.updated = Number(report?.updated || 0);
    scanProgress.skipped = Number(report?.unmatched || 0);
    scanProgress.covers = Number(report?.covers || 0);
    scanProgress.adultIsolated = Number(report?.adultIsolated || 0);
    updateScanProgress('刮削完成，刷新影视库', null);
    await refreshLibraryAfterImport();
    const detail = report ? `已使用 ${enabled} 刮削 ${scanProgress.updated} 条媒体，匹配 ${scanProgress.matched} 条，未匹配 ${scanProgress.skipped} 条，缓存封面 ${scanProgress.covers} 张，隔离 18+ ${scanProgress.adultIsolated} 条。` : '刮削未执行，请检查桌面端连接。';
    finishScanProgress(detail);
    const warned = await maybeWarnTmdbMissing(report);
    if(!warned) toast(detail);
  }catch(error){
    finishScanProgress(backendErrorMessage(error), true);
    toast('刮削失败：' + backendErrorMessage(error));
  }
}

/* 设置页入口：全库重新刮削。跑在扫描进度页上，完成后回到设置。
   每轮最多处理 5000 条未刮削积压；只要仍有进展就持续开下一轮，
   直到积压处理完、剩余条目无法匹配，或用户明确暂停。 */
async function rescrapeLibraryFromSettings(){
  const button = document.getElementById('rescrapeLibraryBtn');
  if(!resetScanProgress('scrape', '全库重新刮削')) return;
  if(button){ button.disabled = true; button.classList.add('loading'); }
  try{
    const providers = TtvBackend.available() ? await TtvBackend.invoke('metadata_providers').catch(() => []) : [];
    const enabled = Array.isArray(providers) ? providers.filter(item => item.enabled).map(item => item.name).join(' + ') : '已启用平台';
    updateScanProgress(`使用 ${enabled} 持续匹配所有未刮削条目…`, null);
    const report = await scrapeLibraryUntilDone(5000, Number.POSITIVE_INFINITY, 'full', false);
    if(scanTaskPausedByUser()) return;
    scanProgress.updated = Number(report?.updated || 0);
    scanProgress.matched = Number(report?.matched || 0);
    scanProgress.skipped = Number(report?.unmatched || 0);
    scanProgress.covers = Number(report?.covers || 0);
    scanProgress.adultIsolated = Number(report?.adultIsolated || 0);
    await refreshLibraryAfterImport();
    const detail = `全库刮削完成：更新 ${scanProgress.updated} 条（匹配 ${scanProgress.matched}，未匹配 ${scanProgress.skipped}）。`;
    finishScanProgress(detail);
    toast(detail);
  }catch(error){
    finishScanProgress(backendErrorMessage(error), true);
    toast('刮削失败：' + backendErrorMessage(error));
  }finally{
    if(button){ button.disabled = false; button.classList.remove('loading'); }
  }
}

/* ================= 统一全局弹窗 (Modal) ============ */
const appModal = document.getElementById('appModal');
const modalTitle = document.getElementById('modalTitle');
const modalBody = document.getElementById('modalBody');
const modalFoot = document.getElementById('modalFoot');

function openModal(title, bodyHtml, footHtml){
  modalTitle.textContent = title;
  modalBody.innerHTML = bodyHtml;
  modalFoot.innerHTML = footHtml;
  appModal.classList.add('open');
}
function closeModal(e){
  if(e && e.target !== appModal && !e.target.classList.contains('modal-close')) return;
  appModal.classList.remove('open');
}

function openDriveModal(name){
  document.getElementById('currentDriveBread').textContent = name;
  const source = SOURCE_CATALOG.find(item => item.id === SOURCE_IDS[name]);
  if(source && !source.implemented){
    openModal(
      name + ' · 适配器状态',
      `<p style="color:var(--text-dim);line-height:1.7">${name} 已纳入 TTV Box 来源目录，但当前版本尚未接入可验证的 ${source.protocol.toUpperCase()} 协议适配器。配置完成后才会启用浏览、播放和挂载，不会写入虚假的连接状态。</p>`,
      `<button class="btn btn-ghost" onclick="closeModal()">关闭</button>`
    );
    return;
  }
  if(source && source.id === 'local'){
    startScanPipeline();
    return;
  }
  if(source && source.id === 'streamhub'){
    closeModal();
    showView('library');
    loadStreamHubResources();
    return;
  }
  if(source && source.id === 'guangya'){
    openModal(
      '光鸭云盘 · OAuth 状态',
      '<p style="color:var(--text-dim);line-height:1.7">后端已接入设备码登录与安全令牌存储。当前逆向资料没有足够的官方文件列表/播放协议，浏览和播放会保持禁用，不会伪造资源。</p>',
      '<button class="btn btn-ghost" onclick="closeModal()">关闭</button>'
    );
    return;
  }
  openModal(
    '挂载云盘与存储 · ' + name,
    `
      <div class="modal-field">
        <label>挂载别名 (Mount Name)</label>
        <input class="modal-input" value="${name} 主盘" id="inpMountName" />
      </div>
      <div class="modal-field">
        <label>服务器端点 / WebDAV URL</label>
        <input class="modal-input" placeholder="https://你的服务器/dav" />
      </div>
      <div class="modal-field">
        <label>认证令牌 (Token / Password)</label>
        <input class="modal-input" type="password" placeholder="输入后仅用于本次测试，不会显示或写入页面" />
      </div>
      <div class="modal-field">
        <label>本地缓存映射目录</label>
        <input class="modal-input" placeholder="选择或输入本地缓存目录" />
      </div>
    `,
    `
      <button class="btn btn-ghost" onclick="closeModal()">取消</button>
      <button class="btn btn-accent" onclick="saveMountDrive('${name}')">测试并立即挂载</button>
    `
  );
}

function saveMountDrive(name){
  closeModal();
  toast(name + ' 的真实挂载适配器尚未接入，未伪造连接结果。');
}

function openServerModal(name){
  openModal(
    '连接媒体服务器 · ' + name,
    `<div class="catalog-empty"><b>${escapeHtml(name)}</b> 的真实适配器尚未接入。当前版本不会伪造连接、用户资料或同步数量；请使用本地媒体库、StreamHub 或 OpenList。</div>`,
    `<button class="btn btn-ghost" onclick="closeModal()">关闭</button>`
  );
}

function saveServerConnect(name){
  closeModal();
  toast(name + ' 的服务器适配器尚未接入，未伪造连接状态或同步数量。');
}

function openNotificationModal(){
  const trigger = document.querySelector('.notification-trigger');
  if(trigger) toggleNotificationPopover({currentTarget: trigger});
}

function openEditModal(){
  if(!selectedMovie) return;
  openModal(
    '编辑影视元数据 · ' + selectedMovie.t,
    `
      <div class="modal-field">
        <label>影视片名</label>
        <input class="modal-input" id="editMovieTitle" value="${escapeHtml(selectedMovie.t)}" />
      </div>
      <div class="modal-field">
        <label>上映年份</label>
        <input class="modal-input" id="editMovieYear" inputmode="numeric" value="${escapeHtml(selectedMovie.y === '—' ? '' : (selectedMovie.y || ''))}" placeholder="未提供" />
      </div>
      <div class="modal-field">
        <label>影视类型</label>
        <input class="modal-input" id="editMovieGenre" value="${escapeHtml((selectedMovie.genres || [selectedMovie.genre]).filter(Boolean).join(', '))}" placeholder="例如 剧情, 科幻" />
      </div>
      <div class="modal-field">
        <label>剧情简介</label>
        <textarea class="modal-input detail-summary-input" id="editMovieSummary" placeholder="暂无简介">${escapeHtml(selectedMovie.summary || '')}</textarea>
      </div>
      <label class="modal-check-row"><input type="checkbox" id="editMovieAdult" ${selectedMovie.adult ? 'checked' : ''}> <span>标记为 18+ 内容</span></label>
    `,
    `
      <button class="btn btn-ghost" onclick="closeModal()">取消</button>
      <button class="btn btn-accent" onclick="saveMovieMetadata()">保存更改</button>
    `
  );
}

async function saveMovieMetadata(){
  const movie = selectedMovie;
  if(!movie) return;
  const title = document.getElementById('editMovieTitle')?.value.trim();
  const yearText = document.getElementById('editMovieYear')?.value.trim() || '';
  const genres = (document.getElementById('editMovieGenre')?.value || '').split(/[,，]/).map(value => value.trim()).filter(Boolean);
  const summary = document.getElementById('editMovieSummary')?.value.trim() || '';
  const adult = Boolean(document.getElementById('editMovieAdult')?.checked);
  if(!title){ toast('片名不能为空。'); return; }
  const parsedYear = /^\d{4}$/.test(yearText) ? Number(yearText) : null;
  const previous = {t: movie.t, y: movie.y, genre: movie.genre, genres: movie.genres, summary: movie.summary, adult: movie.adult, contentRating: movie.contentRating};
  movie.t = title;
  movie.y = parsedYear || '—';
  movie.genre = genres[0] || '未分类';
  movie.genres = genres;
  movie.summary = summary || '暂无简介。';
  movie.adult = adult;
  movie.contentRating = adult ? '18+' : '';
  try{
    const records = Array.isArray(movie.seriesRecords) && movie.seriesRecords.length ? movie.seriesRecords : (movie.record ? [movie.record] : []);
    if(isNativeMediaMode() && records.length){
      const updatedRecords = records.map(record => {
        const payload = record.payload && typeof record.payload === 'object' ? record.payload : {};
        const identity = parseEpisodeIdentity(payload.sourceTitle || record.remotePath || record.title);
        const recordTitle = identity
          ? `${title} · S${String(identity.seasonNumber || 1).padStart(2, '0')}E${String(identity.episodeNumber).padStart(2, '0')}`
          : title;
        return {
          ...record,
          title: recordTitle,
          sortKey: recordTitle.toLocaleLowerCase('zh-CN'),
          year: parsedYear,
          payload: {...payload, genre: movie.genre, genres, summary: movie.summary, adult, adultManual: true, contentRating: adult ? '18+' : ''}
        };
      });
      for(const updatedRecord of updatedRecords){
        await TtvBackend.invoke('library_upsert', {media: updatedRecord});
      }
      const nfoRecord = updatedRecords[0];
      if(nfoRecord?.remotePath && !/^(https?:\/\/)/i.test(String(nfoRecord.remotePath))){
        await TtvBackend.invoke('metadata_nfo_write', {input:{
          mediaId: String(nfoRecord.id),
          fields:{title, year: parsedYear ? String(parsedYear) : '', plot: movie.summary, season: parseEpisodeIdentity(nfoRecord.remotePath)?.seasonNumber ? String(parseEpisodeIdentity(nfoRecord.remotePath).seasonNumber) : undefined, episode: parseEpisodeIdentity(nfoRecord.remotePath)?.episodeNumber ? String(parseEpisodeIdentity(nfoRecord.remotePath).episodeNumber) : undefined}
        }});
      }
      movie.record = updatedRecords[0];
      if(movie.seriesRecords) movie.seriesRecords = updatedRecords;
    }else{
      localStorage.setItem('ttv.catalogOverride.' + movie.id, JSON.stringify({t:movie.t, y:movie.y, genre:movie.genre, genres:movie.genres, summary:movie.summary}));
    }
  }catch(error){
    Object.assign(movie, previous);
    toast('元数据保存失败：' + backendErrorMessage(error));
    return;
  }
  closeModal();
  openDetail(movie);
  renderGrid();
  renderGallery();
  renderWatchlist();
  toast(isNativeMediaMode() ? '影视元数据已写入本地媒体库。' : '目录备注已保存在本机。');
}

function openAboutModal(){
  openModal(
    '关于 TTV Box',
    `
      <div style="text-align:center;padding:10px 0">
        <div style="font-size:36px;margin-bottom:8px">🎬</div>
        <h4 style="font-size:18px;margin:0;color:#fff">TTV Box Cinema OS</h4>
        <p style="color:var(--text-faint);font-size:12px;margin:4px 0 16px">TTV Box · 本地构建 0.1.0</p>
        <p style="color:var(--text-dim);font-size:13px;line-height:1.7;text-align:left">
          用于管理本地媒体、已连接媒体来源和真实播放历史的桌面播放器。视频能力与音轨、画质、硬件加速状态均以当前媒体和本机运行时检测结果为准。
        </p>
      </div>
    `,
    `
      <button class="btn btn-accent" style="width:100%" onclick="closeModal()">确定</button>
    `
  );
}

/* ================= 设置项处理 ============ */
function setThemeAccent(accent, glow, el){
  document.documentElement.style.setProperty('--accent', accent);
  document.documentElement.style.setProperty('--accent-glow', glow);
  document.documentElement.style.setProperty('--accent-2', accent);
  document.querySelectorAll('.theme-dot').forEach(d => d.classList.remove('active'));
  if(el) el.classList.add('active');
  if(TtvBackend.available()) TtvBackend.invoke('settings_set', {key: 'appearance.accent', value: accent}).catch(error => console.warn('Unable to save accent:', error));
  toast('已切换主题色');
}

async function cleanCache(){
  const cacheText = document.getElementById('cacheSizeText');
  if(!TtvBackend.available()){
    toast('当前页面未连接桌面端，无法清理播放缓存。');
    return;
  }
  try{
    const removed = await TtvBackend.invoke('playback_cache_clear');
    if(cacheText) cacheText.textContent = '0.0 MB · 容量未配置';
    toast(removed ? `已清理 ${removed} 个播放缓存目录。` : '没有可清理的播放缓存。');
  }catch(error){
    toast('清理缓存失败：' + backendErrorMessage(error));
  }
}

async function configureSubtitleCredentials(){
  if(!TtvBackend.available()){
    toast('OpenSubtitles 凭据只能在桌面端保存。');
    return;
  }
  openModal('配置 OpenSubtitles', `
    <div style="display:flex;flex-direction:column;gap:10px">
      <p style="color:var(--text-dim);font-size:12px;line-height:1.6;margin:0">API Key 仅通过桌面端安全凭据存储保存，不会写入普通设置或日志。</p>
      <input id="subtitleApiKeyInput" class="search-input" type="password" autocomplete="off" placeholder="粘贴 OpenSubtitles API Key">
    </div>
  `, `
    <button class="btn btn-ghost" onclick="TtvBackend.invoke('subtitle_credentials_clear').then(() => { toast('字幕凭据已清除'); closeModal(); }).catch(error => toast('清除失败：' + backendErrorMessage(error)))">清除</button>
    <button class="btn btn-accent" onclick="saveSubtitleCredentials()">保存</button>
  `);
}

async function saveSubtitleCredentials(){
  const input = document.getElementById('subtitleApiKeyInput');
  const apiKey = input?.value?.trim();
  if(!apiKey){ toast('请输入 API Key。'); return; }
  try{
    await TtvBackend.invoke('subtitle_credentials_set', {input:{apiKey}});
    toast('字幕凭据已安全保存。');
    closeModal();
  }catch(error){
    toast('保存失败：' + backendErrorMessage(error));
  }
}

/* ---- TMDB 元数据密钥（电影刮削加强） ---- */
async function configureTmdbToken(){
  if(!TtvBackend.available()){
    toast('TMDB 密钥只能在桌面端保存。');
    return;
  }
  let statusLine = '尚未配置 TMDB 密钥，电影元数据目前主要依赖 TVMaze（仅剧集）。';
  try{
    const status = await TtvBackend.invoke('metadata_tmdb_status');
    if(status && status.configured){
      statusLine = status.source === 'settings'
        ? '当前已在本应用设置中配置 TMDB 密钥。'
        : '当前正使用环境变量提供的 TMDB 密钥。';
    }
  }catch(error){
    statusLine = '无法读取 TMDB 状态：' + backendErrorMessage(error);
  }
  openModal('配置 TMDB 密钥', `
    <div style="display:flex;flex-direction:column;gap:10px">
      <p style="color:var(--text-dim);font-size:12px;line-height:1.6;margin:0">${escapeHtml(statusLine)}</p>
      <p style="color:var(--text-faint);font-size:11px;line-height:1.6;margin:0">在 themoviedb.org 的 Settings → API 中免费获取 API Key 或 Read Access Token。密钥只保存在本机，不会上传，也不会回显到界面。</p>
      <input id="tmdbTokenInput" class="search-input" type="password" autocomplete="off" placeholder="粘贴 TMDB API Key 或 Read Access Token">
    </div>
  `, `
    <button class="btn btn-ghost" onclick="clearTmdbToken()">清除</button>
    <button class="btn btn-accent" onclick="saveTmdbToken()">保存</button>
  `);
}
async function saveTmdbToken(){
  const input = document.getElementById('tmdbTokenInput');
  const token = input?.value?.trim();
  if(!token){ toast('请输入 TMDB 密钥。'); return; }
  try{
    await TtvBackend.invoke('metadata_tmdb_set', {input:{token}});
    toast('TMDB 密钥已保存，下次刮削即生效。');
    closeModal();
    refreshTmdbStatusCard();
  }catch(error){
    toast('保存失败：' + backendErrorMessage(error));
  }
}
async function clearTmdbToken(){
  try{
    await TtvBackend.invoke('metadata_tmdb_set', {input:{token:''}});
    toast('TMDB 密钥已清除。');
    closeModal();
    refreshTmdbStatusCard();
  }catch(error){
    toast('清除失败：' + backendErrorMessage(error));
  }
}
async function refreshTmdbStatusCard(){
  const el = document.getElementById('tmdbStatusText');
  if(!el) return;
  if(!TtvBackend.available()){ el.textContent = '仅桌面端可用'; return; }
  try{
    const status = await TtvBackend.invoke('metadata_tmdb_status');
    el.textContent = status && status.configured
      ? (status.source === 'settings' ? '已配置（本应用设置）' : '已配置（环境变量）')
      : '未配置';
  }catch(error){
    el.textContent = '状态读取失败';
  }
}

/* ---- 18+ 重新隔离（修复历史遗漏） ---- */
async function reclassifyAdultLibrary(){
  if(!TtvBackend.available()){ toast('18+ 重新隔离只能在桌面端执行。'); return; }
  const button = document.getElementById('reclassifyAdultBtn');
  if(button){ button.disabled = true; button.classList.add('loading'); }
  try{
    const report = await TtvBackend.invoke('library_reclassify_adult');
    await loadInitialCatalog();
    toast(`已重新扫描 ${report?.scanned || 0} 条记录，新隔离 18+ ${report?.flagged || 0} 条。`);
  }catch(error){
    toast('重新隔离失败：' + backendErrorMessage(error));
  }finally{
    if(button){ button.disabled = false; button.classList.remove('loading'); }
  }
}

/* ---- 18+ 重建（清除历史误判，把正常视频移出隔离区） ---- */
async function rebuildAdultLibrary(){
  if(!TtvBackend.available()){ toast('18+ 重建只能在桌面端执行。'); return; }
  const button = document.getElementById('rebuildAdultBtn');
  if(button){ button.disabled = true; button.classList.add('loading'); }
  try{
    const report = await TtvBackend.invoke('library_rebuild_adult');
    await loadInitialCatalog();
    toast(`已重建 ${report?.scanned || 0} 条记录的 18+ 标记，当前隔离 ${report?.flagged || 0} 条；误判的正常视频已移出隔离区。`);
  }catch(error){
    toast('18+ 重建失败：' + backendErrorMessage(error));
  }finally{
    if(button){ button.disabled = false; button.classList.remove('loading'); }
  }
}

/* ---- 18+ 启动一次性自动修复 ----
   分类器升级后重建旧记录，既补漏也清除由历史规则造成的误判。
   每次分类器规则升级时递增 ADULT_AUTO_RECLASSIFY_VERSION，即可对存量库再跑一遍。 */
const ADULT_AUTO_RECLASSIFY_VERSION = 2;
async function autoReclassifyAdultOnce(){
  if(!TtvBackend.available()) return;
  const key = 'ttv.adultAutoReclassify.v' + ADULT_AUTO_RECLASSIFY_VERSION;
  try{
    if(localStorage.getItem(key)) return;
    const report = await TtvBackend.invoke('library_rebuild_adult');
    localStorage.setItem(key, String(Date.now()));
    if(report?.flagged){
      console.info('[adult] one-time rebuild isolated ' + report.flagged + ' item(s) at startup');
    }
  }catch(error){
    // 失败不阻塞启动；下次版本升级或手动点"重新隔离"仍可修复。
    console.warn('One-time adult reclassify skipped:', error);
  }
}

/* ================= 播放器高级控制 ================= */
const player = document.getElementById('view-player');
const playerVideo = document.getElementById('playerVideo');
const pFill = document.getElementById('pFill');
const tCur = document.getElementById('tCur');
const playIcon = document.getElementById('playIcon');
const playerUi = document.getElementById('playerUi');
let playerHls = null;
let playerDash = null;
let playerAudioRestorePending = false;
let TOTAL = 0;
let cur = 0, playing = false, timer = null, hideTimer = null, wasActive = 'home';
let pendingNativeSeek = null;
let pendingNativeSeekAt = 0;
let nativeLoadWaitStartedAt = 0;
let nativeRevealArmedAt = 0;
let nativeFirstFrameStableAt = 0;
let nativePlaybackFallbackInFlight = false;
let nativeSwitchPending = false;
let nativePlayNudgeAt = 0;
let browserVideoLoadId = 0;
// 每次打开/关闭播放器都会推进会话号。异步解析播放地址、创建 libmpv
// 或兼容播放流期间如果页面已经离开播放器，旧请求不得在稍后把播放器重新打开。
let playerSessionId = 0;
let nativeCloseInFlight = null;
let nativeCloseInFlightSession = null;
// 原生画面在短剧里应尽快出现；超过这个窗口就切到浏览器兼容流，避免用户面对长时间黑屏。
const NATIVE_LOAD_TIMEOUT_MS = 6500;
const BROWSER_LOAD_TIMEOUT_MS = 8000;
const NATIVE_CLOSE_TIMEOUT_MS = 1500;
const NATIVE_SWITCH_GRACE_MS = 400;
let watchProgressTimer = null;
let watchProgressDirty = false;
let isDanmakuOn = false, danmakuTimer = null;
let playerVolume = 100;
let playerMuted = false;
let playerVolumeBeforeMute = 100;

let danmakuItems = [];

function fmt(s){
  const h = Math.floor(s/3600), m = Math.floor(s%3600/60), sec = Math.floor(s%60);
  return (h>0 ? String(h).padStart(2,'0')+':' : '') + String(m).padStart(2,'0') + ':' + String(sec).padStart(2,'0');
}
function renderProgress(){
  const progress = TOTAL > 0 ? Math.min(100, Math.max(0, cur / TOTAL * 100)) : 0;
  pFill.style.width = progress + '%';
  tCur.textContent = fmt(cur);
}
let playerPreviewVideo = null;
let playerPreviewSource = '';
let playerPreviewSeekToken = 0;
let playerPreviewRequestTimer = null;
let playerPreviewRequestInFlight = false;
let playerPreviewPendingRequest = null;
const playerPreviewFrameCache = new Map();
let lastKnownVideoSize = {w:0, h:0};
function setPlayerPreviewSource(source){
  playerPreviewSource = String(source || '');
  playerPreviewSeekToken += 1;
  if(!playerPreviewVideo || !playerPreviewSource) return;
  try{
    playerPreviewVideo.pause();
    playerPreviewVideo.removeAttribute('src');
    playerPreviewVideo.load();
  }catch(error){ /* preview is optional */ }
}
function previewCanvas(){ return document.getElementById('playerSeekPreviewCanvas'); }
function seekPreviewBox(){ return document.getElementById('playerSeekPreview'); }
function sizeSeekPreview(srcW, srcH){
  const canvas = previewCanvas();
  const preview = seekPreviewBox();
  if(!canvas || !preview || !(srcW > 0) || !(srcH > 0)) return;
  const portrait = srcH > srcW;
  const maxW = portrait ? 126 : 224;
  const maxH = portrait ? 224 : 126;
  const scale = Math.min(maxW / srcW, maxH / srcH);
  const width = Math.max(72, Math.round(srcW * scale));
  const height = Math.max(72, Math.round(srcH * scale));
  if(canvas.width !== width) canvas.width = width;
  if(canvas.height !== height) canvas.height = height;
  preview.classList.toggle('is-portrait', portrait);
  preview.classList.toggle('is-landscape', !portrait);
  preview.style.width = width + 'px';
  preview.style.height = height + 'px';
}
function drawContainedPreview(source, srcW, srcH){
  const canvas = previewCanvas();
  if(!canvas || !(srcW > 0) || !(srcH > 0)) return false;
  sizeSeekPreview(srcW, srcH);
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = '#05070c';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(source, 0, 0, srcW, srcH, 0, 0, canvas.width, canvas.height);
  const box = seekPreviewBox();
  box?.classList.remove('is-waiting');
  box?.classList.add('has-frame');
  return true;
}
function drawPlayerPreview(video){
  if(!video || !(video.videoWidth > 0 && video.videoHeight > 0)) return false;
  try{
    return drawContainedPreview(video, video.videoWidth, video.videoHeight);
  }catch(error){
    return false;
  }
}
function drawPlayerPreviewImage(dataUrl, token){
  if(!dataUrl) return;
  const image = new Image();
  image.onload = () => {
    const preview = seekPreviewBox();
    if(token !== playerPreviewSeekToken && !preview?.classList.contains('visible')) return;
    drawContainedPreview(image, image.naturalWidth, image.naturalHeight);
  };
  image.onerror = () => {
    if(token === playerPreviewSeekToken) seekPreviewBox()?.classList.remove('is-waiting');
  };
  image.src = dataUrl;
}
function nearestPreviewFrame(positionSeconds){
  let best = null;
  let bestDist = Infinity;
  playerPreviewFrameCache.forEach((dataUrl, key) => {
    const idx = String(key).lastIndexOf(':');
    const pos = Number(String(key).slice(idx + 1));
    if(!Number.isFinite(pos)) return;
    const dist = Math.abs(pos - positionSeconds);
    if(dist < bestDist){
      bestDist = dist;
      best = dataUrl;
    }
  });
  return bestDist <= 8 ? best : null;
}
function rememberVideoSize(width, height){
  const w = Number(width) || 0;
  const h = Number(height) || 0;
  if(w > 0 && h > 0) lastKnownVideoSize = {w, h};
}
function applyKnownPreviewAspect(){
  if(playerVideo && player.classList.contains('has-real-video') && playerVideo.videoWidth > 0){
    rememberVideoSize(playerVideo.videoWidth, playerVideo.videoHeight);
  }
  if(lastKnownVideoSize.w > 0 && lastKnownVideoSize.h > 0){
    sizeSeekPreview(lastKnownVideoSize.w, lastKnownVideoSize.h);
  }
}
function finishPlayerPreviewRequest(token, positionSeconds, dataUrl){
  if(dataUrl){
    drawPlayerPreviewImage(dataUrl, token);
    return;
  }
  const fallback = nearestPreviewFrame(positionSeconds);
  if(fallback) drawPlayerPreviewImage(fallback, token);
  else if(playerVideo && player.classList.contains('has-real-video')) drawPlayerPreview(playerVideo);
  else seekPreviewBox()?.classList.remove('is-waiting');
}
function requestBackendPlayerPreview(target){
  if(!TtvBackend.available() || !selectedMovie?.playUrl) return false;
  const positionSeconds = Math.max(0, Math.round(target * 2) / 2);
  const key = `${selectedMovie.id || selectedMovie.playUrl}:${positionSeconds}`;
  const exact = playerPreviewFrameCache.get(key);
  const nearby = exact || nearestPreviewFrame(positionSeconds);
  const box = seekPreviewBox();
  if(exact){
    drawPlayerPreviewImage(exact, playerPreviewSeekToken);
    box?.classList.remove('is-waiting');
    return true;
  }
  if(nearby) drawPlayerPreviewImage(nearby, playerPreviewSeekToken);
  if(!box?.classList.contains('has-frame')) box?.classList.add('is-waiting');
  playerPreviewPendingRequest = {
    key, positionSeconds,
    url: selectedMovie.playUrl,
    headers: selectedMovie.playHeaders || {},
    decryptionKey: selectedMovie.decryptionKey || undefined,
    mediaId: selectedMovie.id || undefined
  };
  if(playerPreviewRequestInFlight || playerPreviewRequestTimer) return true;
  playerPreviewRequestTimer = window.setTimeout(async () => {
    playerPreviewRequestTimer = null;
    const request = playerPreviewPendingRequest;
    playerPreviewPendingRequest = null;
    if(!request) return;
    const token = ++playerPreviewSeekToken;
    playerPreviewRequestInFlight = true;
    const watchdog = window.setTimeout(() => {
      seekPreviewBox()?.classList.remove('is-waiting');
    }, 1800);
    try{
      const result = await Promise.race([
        TtvBackend.invoke('player_preview_frame', {input:{
          url: request.url,
          headers: request.headers,
          positionSeconds: request.positionSeconds,
          decryptionKey: request.decryptionKey,
          mediaId: request.mediaId
        }}),
        new Promise((_, reject) => window.setTimeout(() => reject(new Error('preview timeout')), 1800))
      ]);
      const dataUrl = result?.dataUrl || result?.data_url;
      if(dataUrl){
        playerPreviewFrameCache.set(request.key, dataUrl);
        if(playerPreviewFrameCache.size > 72) playerPreviewFrameCache.delete(playerPreviewFrameCache.keys().next().value);
      }
      finishPlayerPreviewRequest(token, request.positionSeconds, dataUrl);
    }catch(error){
      console.debug('Unable to capture timeline preview:', error);
      finishPlayerPreviewRequest(token, request.positionSeconds, null);
    }finally{
      window.clearTimeout(watchdog);
      playerPreviewRequestInFlight = false;
      if(playerPreviewPendingRequest) requestBackendPlayerPreview(playerPreviewPendingRequest.positionSeconds);
    }
  }, 50);
  return true;
}
function showPlayerSeekPreview(clientX){
  if(!(TOTAL > 0) || !progressBar) return;
  const preview = document.getElementById('playerSeekPreview');
  const time = document.getElementById('playerSeekPreviewTime');
  if(!preview || !time) return;
  const rect = progressBar.getBoundingClientRect();
  const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
  const target = ratio * TOTAL;
  preview.style.left = `${ratio * 100}%`;
  preview.classList.add('visible');
  time.textContent = fmt(target);
  applyKnownPreviewAspect();
  if(requestBackendPlayerPreview(target)) return;
  const fallback = () => drawPlayerPreview(playerVideo);
  if(!playerPreviewSource || /\.(m3u8|mpd)(?:[?#]|$)/i.test(playerPreviewSource)){
    fallback();
    return;
  }
  if(!playerPreviewVideo){
    playerPreviewVideo = document.createElement('video');
    playerPreviewVideo.muted = true;
    playerPreviewVideo.preload = 'metadata';
    playerPreviewVideo.playsInline = true;
    playerPreviewVideo.crossOrigin = 'anonymous';
  }
  const token = ++playerPreviewSeekToken;
  const capture = () => {
    if(token === playerPreviewSeekToken) drawPlayerPreview(playerPreviewVideo);
  };
  if(playerPreviewVideo.src !== playerPreviewSource){
    playerPreviewVideo.src = playerPreviewSource;
    playerPreviewVideo.addEventListener('loadedmetadata', () => {
      if(token !== playerPreviewSeekToken) return;
      try{ playerPreviewVideo.currentTime = Math.min(Math.max(0, target), Math.max(0, playerPreviewVideo.duration - .1)); }catch(error){ fallback(); }
    }, {once:true});
    playerPreviewVideo.addEventListener('seeked', capture, {once:true});
    playerPreviewVideo.addEventListener('error', fallback, {once:true});
  }else if(playerPreviewVideo.readyState >= 1){
    playerPreviewVideo.addEventListener('seeked', capture, {once:true});
    try{ playerPreviewVideo.currentTime = Math.min(Math.max(0, target), Math.max(0, playerPreviewVideo.duration - .1)); }catch(error){ fallback(); }
  }else{
    fallback();
  }
}
function hidePlayerSeekPreview(){
  const preview = document.getElementById('playerSeekPreview');
  if(!preview) return;
  preview.classList.remove('visible', 'is-waiting');
}
function renderPlayerVolume(){
  const range = document.getElementById('playerVolumeRange');
  const value = document.getElementById('playerVolumeValue');
  const button = document.getElementById('playerMuteBtn');
  const icon = document.getElementById('playerVolumeIcon');
  const control = document.getElementById('playerVolumeControl');
  const volumeBar = document.getElementById('volumeSliderBar');
  const effectiveVolume = playerMuted ? 0 : playerVolume;
  if(range) range.value = String(effectiveVolume);
  // 对标截图：上方纯白数字显示（如 100）
  if(value) value.textContent = String(effectiveVolume);
  if(volumeBar) volumeBar.style.height = effectiveVolume + '%';
  button?.classList.toggle('on', playerMuted);
  control?.classList.toggle('muted', playerMuted);
  if(icon){
    icon.innerHTML = playerMuted || effectiveVolume === 0
      ? '<path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M23 9l-6 6M17 9l6 6"/>'
      : effectiveVolume < 50
        ? '<path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>'
        : '<path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07"/>';
  }
}

/* 垂直音量条手势拖拽引擎 */
let isVolumeDragging = false;
function applyVolumeByClientY(clientY){
  const track = document.getElementById('playerVolumeTrack');
  if(!track) return;
  const rect = track.getBoundingClientRect();
  const clampedY = Math.max(rect.top, Math.min(rect.bottom, clientY));
  // 自下而上：底部为0%，顶部为100%
  const ratio = Math.max(0, Math.min(1, (rect.bottom - clampedY) / rect.height));
  const newVol = Math.round(ratio * 100);
  setPlayerVolume(newVol, false);
}

function startVolumeDrag(e){
  e.preventDefault();
  e.stopPropagation();
  isVolumeDragging = true;
  const track = document.getElementById('playerVolumeTrack');
  const anchor = document.getElementById('playerVolumeControl');
  if(track) track.classList.add('dragging');
  if(anchor) anchor.classList.add('active-hover');
  applyVolumeByClientY(e.clientY);

  function onPointerMove(ev){
    if(!isVolumeDragging) return;
    applyVolumeByClientY(ev.clientY);
  }

  function onPointerUp(){
    isVolumeDragging = false;
    if(track) track.classList.remove('dragging');
    if(anchor) anchor.classList.remove('active-hover');
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
    window.removeEventListener('pointercancel', onPointerUp);
  }

  window.addEventListener('pointermove', onPointerMove);
  window.addEventListener('pointerup', onPointerUp);
  window.addEventListener('pointercancel', onPointerUp);
}
function setPlayerVolume(value, notify = false){
  const next = Math.max(0, Math.min(100, Number(value) || 0));
  playerVolume = next;
  if(next > 0){
    playerVolumeBeforeMute = next;
    playerMuted = false;
  }else{
    playerMuted = true;
  }
  if(playerVideo && player.classList.contains('has-real-video')){
    playerVideo.volume = playerVolume / 100;
    playerVideo.muted = playerMuted;
  }else{
    sendPlayerCommand({type:'setVolume', volume:playerMuted ? 0 : playerVolume});
  }
  renderPlayerVolume();
  if(notify) toast(playerMuted ? '已静音' : '音量 ' + playerVolume + '%');
}
function togglePlayerMute(){
  if(playerMuted || playerVolume === 0){
    playerMuted = false;
    playerVolume = Math.max(1, playerVolumeBeforeMute || 70);
  }else{
    playerVolumeBeforeMute = playerVolume;
    playerMuted = true;
  }
  if(playerVideo && player.classList.contains('has-real-video')){
    playerVideo.volume = playerVolume / 100;
    playerVideo.muted = playerMuted;
  }else{
    sendPlayerCommand({type:'setVolume', volume:playerMuted ? 0 : playerVolume});
  }
  renderPlayerVolume();
  uiActivity();
  toast(playerMuted ? '已静音' : '已恢复声音');
}
let playerLoadLogLines = [];
function resetPlayerLoadLog(){
  playerLoadLogLines = [];
  const log = document.getElementById('playerLoadingLog');
  if(log) log.innerHTML = '';
}
function pushPlayerLoadLog(message){
  const text = String(message || '').trim();
  if(!text) return;
  if(playerLoadLogLines[playerLoadLogLines.length - 1] === text) return;
  playerLoadLogLines.push(text);
  if(playerLoadLogLines.length > 6) playerLoadLogLines.shift();
  const log = document.getElementById('playerLoadingLog');
  if(log){
    log.innerHTML = playerLoadLogLines.map(line => `<li>${escapeHtml(line)}</li>`).join('');
    log.scrollTop = log.scrollHeight;
  }
}
function setPlayerLoading(active, message = '正在连接…'){
  const state = document.getElementById('playerLoadingState');
  const label = document.getElementById('playerLoadingText');
  if(message){
    if(label) label.textContent = message;
    if(active) pushPlayerLoadLog(message);
  }
  state?.classList.toggle('active', Boolean(active));
}
function withTimeout(promise, ms, label){
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error(label || '操作超时')), Math.max(1, Number(ms) || 0));
    Promise.resolve(promise).then(
      value => { window.clearTimeout(timer); resolve(value); },
      error => { window.clearTimeout(timer); reject(error); }
    );
  });
}
function movieRequiresNativeDecoder(movie){
  if(!movie) return false;
  if(String(movie.decryptionKey || '').trim()) return true;
  const url = String(movie.playUrl || '');
  return /\.(mkv|avi|ts|m2ts|flv|rm|rmvb|wmv)(?:[?#]|$)/i.test(url);
}
function movieCanPlayInBrowser(movie){
  if(!movie || movieRequiresNativeDecoder(movie)) return false;
  const url = String(movie.browserPlayUrl || movie.playUrl || '');
  if(!url) return false;
  if(isLocalMediaSource(url) && !movie.forceWebPlayback) return false;
  if(movie.forceWebPlayback) return true;
  const headers = movie.playHeaders && typeof movie.playHeaders === 'object' ? movie.playHeaders : {};
  const isAdaptive = /\.(m3u8|mpd)(?:[?#]|$)/i.test(url);
  // <video src> 带不上 Referer/UA/Authorization。有自定义头的渐进式直链必须走 libmpv。
  const needsCustomHeaders = Object.keys(headers).some(name => {
    const key = String(name).toLowerCase();
    return key !== 'origin';
  });
  if(needsCustomHeaders && !isAdaptive) return false;
  if(isAdaptive || /\.(mp4|webm|m4v|mov)(?:[?#]|$)/i.test(url)) return true;
  if(isHongguoPlaybackId(movie.id) && /^https?:/i.test(url) && !needsCustomHeaders) return true;
  return /^https?:/i.test(url) && !isLocalMediaSource(url);
}

function nativePlaybackTimeoutReason(state){
  const backendReason = String(state?.error || '').trim();
  if(backendReason) return backendReason;
  if(state?.status === 'loading') return '视频源没有完成加载';
  if(state?.status === 'buffering') return '视频源持续缓冲';
  return '视频首帧没有在规定时间内准备好';
}

function isPlayerSessionActive(sessionId){
  return sessionId === playerSessionId && Boolean(player?.classList.contains('active'));
}

function discardStalePlayerSession(sessionId){
  if(isPlayerSessionActive(sessionId)) return false;
  // An async HTML5/HLS/DASH load may finish after closePlayer() already ran.
  // Reset the media element once more so that late completion cannot resurrect
  // audio in the background.
  resetPlayerVideo();
  return true;
}

function closeNativePlayback(sessionId = playerSessionId){
  if(!TtvBackend.available()) return Promise.resolve();
  if(nativeCloseInFlight){
    if(nativeCloseInFlightSession === sessionId) return nativeCloseInFlight;
    // A stale close may still be draining. Wait for it, then issue a close for
    // the newer session instead of accidentally reusing the old token.
    return nativeCloseInFlight.then(() => closeNativePlayback(sessionId));
  }
  nativeCloseInFlightSession = sessionId;
  nativeCloseInFlight = withTimeout(
    TtvBackend.invoke('player_native_close', {input:{sessionId}}),
    NATIVE_CLOSE_TIMEOUT_MS,
    '关闭原生播放器超时'
  )
    .catch(error => {
      console.warn('Unable to close native player:', error);
    })
    .finally(() => {
      nativeCloseInFlight = null;
      nativeCloseInFlightSession = null;
    });
  return nativeCloseInFlight;
}

async function recoverNativePlaybackAfterTimeout(movie, state){
  if(nativePlaybackFallbackInFlight || !movie?.playUrl || !isNativeMediaMode()) return;
  if(!player?.classList.contains('active') || String(selectedMovie?.id) !== String(movie.id)) return;

  const sessionId = playerSessionId;
  // CENC / MKV 等只能走 libmpv。杀掉正在出画的原生会话再切浏览器，只会停在
  // 「正在切换兼容播放」且永远播不出来。补帧依赖 libmpv 滤镜链，超时也不能
  // 切到 HTML5，否则开关看起来开了、画面其实没有插帧。
  if(movieRequiresNativeDecoder(movie) || !movieCanPlayInBrowser(movie)
     || enhancementRifeEnabled || movie.forceNativePlayback){
    nativeLoadWaitStartedAt = Date.now();
    setPlayerLoading(true, enhancementRifeEnabled ? '正在启动补帧…' : '网络较慢，继续加载…');
    return;
  }

  nativePlaybackFallbackInFlight = true;
  stopTimer();
  const reason = nativePlaybackTimeoutReason(state);
  setPlayerLoading(true, '原生播放器响应较慢，正在切换兼容播放…');
  try{
    const browserMovie = movie.browserPlayUrl ? {...movie, playUrl: movie.browserPlayUrl} : movie;
    let browserLoaded = false;
    try{
      browserLoaded = await loadBrowserVideo(browserMovie);
    }catch(error){
      console.warn('Browser-compatible recovery failed:', error);
    }
    if(discardStalePlayerSession(sessionId)) return;

    if(browserLoaded){
      document.documentElement.classList.remove('native-video-live');
      player.classList.remove('native-video-live');
      nativeSwitchPending = false;
      void closeNativePlayback(sessionId);
      nativeLoadWaitStartedAt = 0;
      await restoreWatchPosition(movie);
      setPlayerLoading(false);
      toast('原生播放加载超时，已切换兼容播放');
      uiActivity();
      return;
    }

    if(!isHongguoPlaybackId(movie.id) && (isLocalMediaSource(movie.playUrl) || /^https?:\/\//i.test(String(movie.playUrl || '')))){
      const prepared = await withTimeout(
        TtvBackend.invoke('player_prepare_browser_media', {
          input: {url: movie.playUrl, headers: movie.playHeaders || {}}
        }),
        12000,
        '兼容转码超时'
      ).catch(error => {
        console.warn('Browser-compatible recovery preparation failed:', error);
        return null;
      });
      if(discardStalePlayerSession(sessionId)) return;
      if(prepared?.url){
        browserLoaded = await loadBrowserVideo({...movie, playUrl: prepared.url}).catch(() => false);
        if(discardStalePlayerSession(sessionId)) return;
        if(browserLoaded){
          movie.browserPlayUrl = prepared.url;
          document.documentElement.classList.remove('native-video-live');
          player.classList.remove('native-video-live');
          nativeSwitchPending = false;
          void closeNativePlayback(sessionId);
          nativeLoadWaitStartedAt = 0;
          await restoreWatchPosition(movie);
          setPlayerLoading(false);
          toast('原生播放加载超时，已切换兼容播放');
          uiActivity();
          return;
        }
      }
    }

    nativeLoadWaitStartedAt = Date.now();
    setPlayerLoading(true, '网络较慢，继续加载…');
    startTimer();
  }catch(error){
    nativeLoadWaitStartedAt = Date.now();
    setPlayerLoading(true, '网络较慢，继续加载…');
    startTimer();
    console.warn('Compatibility fallback aborted:', reason, error);
  }finally{
    nativePlaybackFallbackInFlight = false;
  }
}
function resetPlayerVideo(){
  if(!playerVideo) return;
  if(playerHls){
    playerHls.destroy();
    playerHls = null;
  }
  if(playerDash){
    try{ playerDash.reset(); }catch(error){ console.warn('Unable to reset DASH player:', error); }
    playerDash = null;
  }
  playerVideo.pause();
  playerVideo.muted = playerMuted;
  playerVideo.volume = playerVolume / 100;
  playerAudioRestorePending = false;
  playerVideo.removeAttribute('src');
  playerVideo.load();
  player.classList.remove('has-real-video');
}
async function loadBrowserVideo(movie){
  if(!playerVideo || !movie?.playUrl) return false;
  setPlayerLoading(true, '正在打开兼容播放…');
  const loadId = ++browserVideoLoadId;
  const sessionId = playerSessionId;
  const stillCurrent = () => loadId === browserVideoLoadId && sessionId === playerSessionId && player?.classList.contains('active');
  if(playerHls){
    playerHls.destroy();
    playerHls = null;
  }
  if(playerDash){
    try{ playerDash.reset(); }catch(error){ console.warn('Unable to reset DASH player:', error); }
    playerDash = null;
  }
  playerVideo.muted = playerMuted;
  playerVideo.volume = playerVolume / 100;
  playerVideo.poster = movie.img || '';
  playerVideo.preload = 'auto';
  const previewUrl = toVideoPreviewUrl(movie.playUrl);
  if(!previewUrl) return false;
  setPlayerPreviewSource(previewUrl);
  player.classList.add('has-real-video');
  const failLoad = () => {
    if(loadId !== browserVideoLoadId) return false;
    if(playerHls){ playerHls.destroy(); playerHls = null; }
    if(playerDash){
      try{ playerDash.reset(); }catch(error){ console.warn('Unable to reset DASH player:', error); }
      playerDash = null;
    }
    player.classList.remove('has-real-video');
    return false;
  };
  if(/\.m3u8(?:[?#]|$)/i.test(previewUrl) || /^blob:/i.test(previewUrl)){
    const HlsCtor = window.TtvHls || (window.TtvLoadHls ? await window.TtvLoadHls() : null);
    if(!stillCurrent()) return false;
    if(!HlsCtor || !HlsCtor.isSupported()) return failLoad();
    return await new Promise((resolve) => {
      let settled = false;
      const finish = (ok) => {
        if(settled) return;
        settled = true;
        if(!stillCurrent()){
          resolve(false);
          return;
        }
        if(!ok) failLoad();
        resolve(ok);
      };
      const headers = movie.playHeaders && typeof movie.playHeaders === 'object' ? movie.playHeaders : {};
      playerHls = new HlsCtor({
        enableWorker: true,
        lowLatencyMode: false,
        backBufferLength: 30,
        maxBufferLength: 20,
        fragLoadingTimeOut: BROWSER_LOAD_TIMEOUT_MS,
        manifestLoadingTimeOut: BROWSER_LOAD_TIMEOUT_MS,
        xhrSetup: (xhr) => Object.entries(headers).forEach(([name, value]) => xhr.setRequestHeader(name, String(value)))
      });
      playerHls.on(HlsCtor.Events.MANIFEST_PARSED, async () => {
        if(!stillCurrent()) return finish(false);
        try{
          await playerVideo.play();
        }catch(error){
          console.warn('HLS autoplay needs a fresh user gesture:', error);
          playerAudioRestorePending = true;
          playing = false;
          setPlayIcon();
          toast('视频已就绪，点击播放按钮即可带声音播放。');
          return finish(true);
        }
        playing = true;
        setPlayIcon();
        startTimer();
        finish(true);
      });
      playerHls.on(HlsCtor.Events.ERROR, (_event, data) => {
        if(data?.fatal){
          console.warn('HLS fatal error:', data);
          finish(false);
        }
      });
      playerHls.loadSource(previewUrl);
      playerHls.attachMedia(playerVideo);
      window.setTimeout(() => { if(stillCurrent()) finish(false); }, BROWSER_LOAD_TIMEOUT_MS);
    });
  }
  if(/\.mpd(?:[?#]|$)/i.test(previewUrl)){
    const DashCtor = window.TtvDash || (window.TtvLoadDash ? await window.TtvLoadDash() : null);
    if(!stillCurrent()) return false;
    if(!DashCtor || typeof DashCtor.MediaPlayer !== 'function') return failLoad();
    return await new Promise((resolve) => {
      let settled = false;
      const finish = (ok) => {
        if(settled) return;
        settled = true;
        if(!stillCurrent()){
          resolve(false);
          return;
        }
        if(!ok) failLoad();
        resolve(ok);
      };
      playerDash = DashCtor.MediaPlayer().create();
      const headers = movie.playHeaders && typeof movie.playHeaders === 'object' ? movie.playHeaders : {};
      if(Object.keys(headers).length && playerDash.extend){
        playerDash.extend('RequestModifier', () => ({
          modifyRequestHeader: (xhr) => { Object.entries(headers).forEach(([name, value]) => xhr.setRequestHeader(name, String(value))); return xhr; },
          modifyRequestURL: (url) => url
        }), true);
      }
      playerDash.on(DashCtor.MediaPlayer.events.STREAM_INITIALIZED, async () => {
        if(!stillCurrent()) return finish(false);
        TOTAL = Number.isFinite(playerVideo.duration) && playerVideo.duration > 0 ? playerVideo.duration : (Number(movie.durationSeconds) || TOTAL);
        try{ await playerVideo.play(); }catch(error){ playerAudioRestorePending = true; playing = false; setPlayIcon(); }
        playing = !playerVideo.paused;
        setPlayIcon();
        startTimer();
        finish(true);
      });
      playerDash.on(DashCtor.MediaPlayer.events.ERROR, () => finish(false));
      playerDash.initialize(playerVideo, previewUrl, true);
      window.setTimeout(() => { if(stillCurrent()) finish(false); }, BROWSER_LOAD_TIMEOUT_MS);
    });
  }
  try{
    if(playerVideo.src !== previewUrl){
      playerVideo.src = previewUrl;
      playerVideo.load();
    }
    await new Promise((resolve, reject) => {
      if(playerVideo.readyState >= 1 && playerVideo.src === previewUrl){
        resolve();
        return;
      }
      let settled = false;
      const finish = (error) => {
        if(settled) return;
        settled = true;
        playerVideo.removeEventListener('loadedmetadata', onReady);
        playerVideo.removeEventListener('error', onError);
        error ? reject(error) : resolve();
      };
      const onReady = () => finish();
      const onError = () => finish(new Error('视频无法在桌面窗口中解码'));
      playerVideo.addEventListener('loadedmetadata', onReady, {once:true});
      playerVideo.addEventListener('error', onError, {once:true});
      window.setTimeout(() => finish(new Error('视频加载超时')), BROWSER_LOAD_TIMEOUT_MS);
    });
  }catch(error){
    console.warn('HTML5 video load failed:', error);
    return failLoad();
  }
  if(!stillCurrent()) return false;
  TOTAL = Number.isFinite(playerVideo.duration) && playerVideo.duration > 0 ? playerVideo.duration : (Number(movie.durationSeconds) || TOTAL);
  cur = 0;
  try{ playerVideo.currentTime = 0; }catch(error){ /* ignore */ }
  try{
    await playerVideo.play();
  }catch(error){
    console.warn('Video autoplay needs a fresh user gesture:', error);
    playerAudioRestorePending = true;
    playing = false;
    setPlayIcon();
    toast('视频已就绪，点击播放按钮即可带声音播放。');
    return true;
  }
  playing = !playerVideo.paused;
  renderProgress();
  document.getElementById('tTotal').textContent = fmt(TOTAL);
  setPlayIcon();
  return true;
}

function restorePlayerAudio(){
  if(!playerVideo || !player.classList.contains('has-real-video')) return;
  playerVideo.muted = playerMuted;
  playerVideo.volume = playerVolume / 100;
  playerAudioRestorePending = false;
  if(playerVideo.paused && playing){
    playerVideo.play().catch(error => {
      console.warn('Unable to restore player audio:', error);
    });
  }
}

function isLocalMediaSource(value){
  const text = String(value || '').trim();
  return Boolean(text) && !/^(https?:|blob:|data:|asset:)/i.test(text);
}

playerVideo?.addEventListener('play', () => {
  playing = true;
  setPlayIcon();
  startTimer();
});
player?.addEventListener('pointerdown', () => {
  if(playerAudioRestorePending) restorePlayerAudio();
}, {capture: true});
player?.addEventListener('keydown', () => {
  if(playerAudioRestorePending) restorePlayerAudio();
}, {capture: true});
playerVideo?.addEventListener('pause', () => {
  playing = false;
  setPlayIcon();
  stopTimer();
});
playerVideo?.addEventListener('timeupdate', () => {
  if(!playerVideo || !Number.isFinite(playerVideo.currentTime)) return;
  cur = playerVideo.currentTime;
  if(Number.isFinite(playerVideo.duration) && playerVideo.duration > 0) TOTAL = playerVideo.duration;
  renderProgress();
  queueWatchProgressSave();
  maybeArmShortDramaTailCountdown();
});
playerVideo?.addEventListener('ended', () => {
  playing = false;
  setPlayIcon();
  stopTimer();
  saveWatchProgress();
  // 短剧连播：当前集播完且仍是短剧会话时，自动解析并打开下一集。
  maybeAutoAdvanceShortDrama();
});
playerVideo?.addEventListener('pause', () => {
  queueWatchProgressSave(true);
});
playerVideo?.addEventListener('error', () => {
  const mediaError = playerVideo.error;
  const reason = mediaError ? ` (code ${mediaError.code})` : '';
  console.warn('HTML5 video error', mediaError);
  toast('当前视频格式无法由 WebView2 直接解码' + reason + '，正在尝试桌面播放器');
});
function syncPlayerContent(movie){
  const m = movie || (currentView === 'home' ? homeMovieAt(current) : null) || selectedMovie;
  if(!m) return;
  selectedMovie = m;
  document.getElementById('pBackTitle').textContent = m.t;
  document.getElementById('pInfoTitle').textContent = m.t;
  document.getElementById('pInfoDesc').textContent = m.summary;
  document.getElementById('pTitle').textContent = m.t;
  const sourceTag = document.getElementById('pSourceTag');
  if(sourceTag){
    sourceTag.textContent = appMode !== 'catalog' ? (m.q || '4K') : (m.network || 'TVMaze');
  }
  const sourceText = document.getElementById('pSource');
  if(sourceText){
    sourceText.textContent = appMode !== 'catalog'
      ? (m.sourceLabel || '本地媒体') + ' · ' + (m.playUrl ? '正在使用桌面播放器加载真实文件' : '正在解析可播放资源')
      : '公开目录预览 · ' + (m.network || 'TVMaze') + ' · 连接媒体源后可播放';
  }
  const sourceName = basename(m.playUrl || m.sourceTitle || m.providerMediaId || '');
  const extension = sourceName.includes('.') ? sourceName.split('.').pop().toUpperCase() : '';
  const qualityLabel = m.playbackQuality || (extension ? extension : (m.q || '自动'));
  const qualityChip = document.getElementById('chipQuality');
  if(qualityChip) qualityChip.textContent = qualityLabel;
  const audioMode = document.getElementById('pAudioModeText');
  const audioDevice = document.getElementById('pAudioDeviceText');
  const channelMap = document.getElementById('pChannelMapText');
  if(audioMode) audioMode.textContent = '自动选择可用音轨';
  if(audioDevice) audioDevice.textContent = '系统默认输出设备';
  if(channelMap) channelMap.textContent = '跟随媒体源';
  document.getElementById('playerBackdrop').style.backgroundImage = `linear-gradient(90deg,rgba(10,12,16,.18),rgba(10,12,16,.48)),url("${m.img}")`;
  document.getElementById('pInfoPoster').style.backgroundImage = `linear-gradient(180deg,transparent 45%,rgba(10,12,16,.92)),url("${m.img}")`;
  const previewCanvas = document.getElementById('playerSeekPreviewCanvas');
  if(previewCanvas) previewCanvas.style.backgroundImage = '';
}

/* ============ ScrollExpand 全局卡片物理位置全屏扩张动效引擎 ============ */
let isPlayerExpanding = false;

function openHeroExpandPlayer(btn){
  if(!homeMovieAt(current) && !MOVIES[0]) return toast('当前没有可播放媒体，请先扫描并选择目录。');
  openPlayer(homeMovieAt(current) || MOVIES[0], btn);
}

async function restoreWatchPosition(movie){
  if(!isNativeMediaMode() || !movie?.id) return;
  try{
    const history = await TtvBackend.invoke('history_get', {mediaId: String(movie.id)});
    if(history){
      TOTAL = Math.max(1, Number(history.durationSeconds) || Number(movie.durationSeconds) || TOTAL);
      const savedPosition = Math.min(TOTAL, Math.max(0, Number(history.positionSeconds) || 0));
      // 已完整播完的记录不能再作为续播位置，否则每次打开都会被 seek 到片尾，
      // 画面停黑、进度条满格，看起来像“所有视频都无法播放”。
      const completed = history.completed === true || (TOTAL > 0 && savedPosition / TOTAL >= 0.95);
      cur = completed ? 0 : savedPosition;
      if(playerVideo && player.classList.contains('has-real-video')) playerVideo.currentTime = cur;
      else if(cur > 0 && TtvBackend.available()){
        await TtvBackend.invoke('player_command', {command:{type:'seek', positionSeconds:cur}}).catch(error => {
          console.warn('Unable to restore native playback position:', error);
        });
      }
      renderProgress();
    }
  }catch(error){
    console.warn('Unable to restore watch history:', error);
  }
}
async function syncBackendPlayback(){
  if(!isNativeMediaMode() || !selectedMovie?.playUrl) return;
  if(playerVideo && player.classList.contains('has-real-video')){
    if(Number.isFinite(playerVideo.duration) && playerVideo.duration > 0) TOTAL = playerVideo.duration;
    if(Number.isFinite(playerVideo.currentTime)) cur = playerVideo.currentTime;
    playing = !playerVideo.paused && !playerVideo.ended;
    document.getElementById('tTotal').textContent = fmt(TOTAL);
    setPlayIcon();
    renderProgress();
    return;
  }
  try{
    const state = await TtvBackend.invoke('player_state');
    if(state.mediaId && state.mediaId !== String(selectedMovie.id)){
      if(nativeSwitchPending && nativeLoadWaitStartedAt
         && Date.now() - nativeLoadWaitStartedAt >= NATIVE_SWITCH_GRACE_MS){
        setPlayerLoading(true, '正在切换…');
      }
      return;
    }
    if(Number.isFinite(state.durationSeconds) && state.durationSeconds > 0){
      TOTAL = state.durationSeconds;
    }else if(!(TOTAL > 0) && Number(selectedMovie.durationSeconds) > 0){
      TOTAL = Number(selectedMovie.durationSeconds);
    }
    if(Number.isFinite(state.positionSeconds)){
      const backendPosition = Math.min(TOTAL, Math.max(0, state.positionSeconds));
      const seekStillPending = pendingNativeSeek !== null && Date.now() - pendingNativeSeekAt < 900;
      if(seekStillPending){
        cur = pendingNativeSeek;
      }else{
        pendingNativeSeek = null;
        cur = backendPosition;
      }
    }
    if(Number.isFinite(state.volume)){
      playerVolume = Math.max(0, Math.min(100, Number(state.volume)));
      playerMuted = playerVolume <= 0;
      if(playerVolume > 0) playerVolumeBeforeMute = playerVolume;
      renderPlayerVolume();
    }
    // Loading/FILE_LOADED 还没有真正出首帧时，不要提前显示暂停图标。
    // 只有 mpv 已确认首帧，或时间位置确实开始前进，才算用户可见的播放中。
    if(Number(state.videoWidth) > 0 && Number(state.videoHeight) > 0){
      rememberVideoSize(state.videoWidth, state.videoHeight);
    }
    const nativeFrameReady = state.firstFrameReady === true
      || (Number.isFinite(state.positionSeconds) && Number(state.positionSeconds) > 0.04);
    playing = nativeFrameReady
      && (state.status === 'playing' || state.status === 'buffering');
    // libmpv 可能在 playback-restart 后才报告解码错误；此时 native-video-live
    // 已被置位，错误处理不能藏在“尚未出画”的分支里，否则会留下纯黑画面。
    if(state.status === 'error'){
      nativeLoadWaitStartedAt = 0;
      if(isHongguoPlaybackId(selectedMovie?.id) && !selectedMovie?.shortDramaStreamFallback){
        void recoverShortDramaNativePlayback(selectedMovie);
        return;
      }
      setPlayerLoading(false);
      stopTimer();
      const sourceText = document.getElementById('pSource');
      const errorMessage = state.error || '未知错误';
      if(sourceText) sourceText.textContent = '播放失败 · ' + errorMessage;
      toast('无法加载此文件：' + errorMessage);
      closePlayer();
      return;
    }
    if(state.status === 'ended' && !state.firstFrameReady && isHongguoPlaybackId(selectedMovie?.id)
       && !selectedMovie?.shortDramaStreamFallback){
      void recoverShortDramaNativePlayback(selectedMovie);
      return;
    }
    // 已出首帧后播放器会保留 native-video-live 类名；结束检测必须放在该分支外，
    // 否则 mpv 正常播放到结尾时不会触发短剧下一集。
    if(state.status === 'ended') maybeAutoAdvanceShortDrama();
    // 切集复用 libmpv 时仍保留 native-video-live。新文件的首帧信号到来前
    // 继续快轮询，超时只对浏览器可播的片源做兼容回退。
    if(player.classList.contains('native-video-live') && nativeSwitchPending){
      const sameMedia = String(state.mediaId || '') === String(selectedMovie.id);
      if(sameMedia && state.status === 'paused' && Date.now() - nativePlayNudgeAt > 800){
        nativePlayNudgeAt = Date.now();
        sendPlayerCommand({type:'play'});
      }
      const switched = sameMedia
        && state.status !== 'loading'
        && (state.firstFrameReady === true
          || (Number.isFinite(state.positionSeconds) && Number(state.positionSeconds) > 0.04));
      if(switched){
        nativeSwitchPending = false;
        nativeFirstFrameStableAt = Date.now();
        setPlayerLoading(false);
        startTimer();
      }else if(nativeLoadWaitStartedAt && Date.now() - nativeLoadWaitStartedAt >= NATIVE_SWITCH_GRACE_MS){
        setPlayerLoading(true, '正在切换…');
        if(Date.now() - nativeLoadWaitStartedAt >= NATIVE_LOAD_TIMEOUT_MS && !nativePlaybackFallbackInFlight){
          void recoverNativePlaybackAfterTimeout(selectedMovie, state);
        }
      }
    }
    // mpv 嵌入在 WebView2 之下：文件加载完成前保持不透明加载画面，
    // 确认首帧就绪后再把画布变透明让视频透出，避免加载期间"透视窗口"。
    if(!player.classList.contains('native-video-live')){
      if(state.status === 'loading'){
        setPlayerLoading(true, '正在读取媒体…');
      }else if(state.status === 'buffering' && !state.firstFrameReady){
        const buffered = Number(state.bufferedPercent);
        setPlayerLoading(true, Number.isFinite(buffered) ? `正在缓冲 ${Math.max(0, Math.min(100, Math.round(buffered)))}%` : '正在缓冲…');
      }
      if(nativeLoadWaitStartedAt && Date.now() - nativeLoadWaitStartedAt >= NATIVE_LOAD_TIMEOUT_MS
         && !state.firstFrameReady && !nativePlaybackFallbackInFlight){
        void recoverNativePlaybackAfterTimeout(selectedMovie, state);
      }
      const statusActive = state.status === 'playing' || state.status === 'paused'
        || state.status === 'buffering' || state.status === 'ended';
      if(!statusActive){
        nativeRevealArmedAt = 0;
        if(nativeLoadWaitStartedAt && Date.now() - nativeLoadWaitStartedAt > 8000){
          setPlayerLoading(true, '网络较慢，继续加载…');
        }
        return;
      }
      // 真正能点透明画布的是"首帧已渲染"信号（后端 firstFrameReady，来自 mpv
      // playback-restart）。status 在 FILE_LOADED 就翻成 playing/buffering，早于首帧
      // 绘出——尤其补帧/超分要加载 ONNX 模型、GLSL 要编译着色器时首帧会晚好几秒，
      // 只按 status 清空画布就会在出画前透视窗口。
      const firstFrameSignal = state.firstFrameReady === true || (
        Number.isFinite(state.positionSeconds)
        && Number(state.positionSeconds) > 0.04
        && (state.status === 'playing' || state.status === 'paused' || state.status === 'buffering')
      );
      if(firstFrameSignal && !nativeFirstFrameStableAt){
        nativeFirstFrameStableAt = Date.now();
      }
      if(!firstFrameSignal) nativeFirstFrameStableAt = 0;
      // 保持首帧信号至少一小段时间后才清空 WebView 画布；这样可避开
      // mpv 刚报 playback-restart、DWM 尚未合成画面的透明闪屏。
      const reveal = nativeFirstFrameStableAt > 0 && Date.now() - nativeFirstFrameStableAt >= 90;
      if(!reveal){
        if(nativeLoadWaitStartedAt && Date.now() - nativeLoadWaitStartedAt > 8000){
          setPlayerLoading(true, '网络较慢，继续加载…');
        }
        return;
      }
      document.documentElement.classList.add('native-video-live');
      player.classList.add('native-video-live');
      nativeSwitchPending = false;
      pushPlayerLoadLog('首帧已就绪');
      setPlayerLoading(false);
      startTimer(); // 已出画：从 250ms 快速轮询切回 1s 常规心跳
    }
    updateEnhancementFeedback(state);
    document.getElementById('tTotal').textContent = fmt(TOTAL);
    setPlayIcon();
    renderProgress();
    maybeArmShortDramaTailCountdown();
  }catch(error){
    console.warn('Unable to read player state:', error);
  }
}
async function saveWatchProgress(){
  if(!isNativeMediaMode() || !selectedMovie?.id || !selectedMovie.playUrl) return;
  watchProgressDirty = false;
  try{
    await TtvBackend.invoke('history_save', {
      mediaId: String(selectedMovie.id),
      positionSeconds: cur,
      durationSeconds: TOTAL,
      completed: TOTAL > 0 && cur / TOTAL >= 0.95
    });
  }catch(error){
    console.warn('Unable to save watch history:', error);
  }
}
function queueWatchProgressSave(immediate = false){
  if(!isNativeMediaMode() || !selectedMovie?.id || !selectedMovie.playUrl) return;
  watchProgressDirty = true;
  if(immediate){
    if(watchProgressTimer){ clearTimeout(watchProgressTimer); watchProgressTimer = null; }
    saveWatchProgress();
    return;
  }
  if(watchProgressTimer) return;
  watchProgressTimer = setTimeout(() => {
    watchProgressTimer = null;
    if(watchProgressDirty) saveWatchProgress();
  }, 8000);
}
window.addEventListener('pagehide', () => {
  if(watchProgressDirty) saveWatchProgress();
  saveScanTasks();
  // WebView2 发生渲染器重载/路由替换时，页面的播放器 DOM 会消失，但
  // 原生 libmpv actor 不会自动跟随 DOM 生命周期。带上当前会话号发起关闭，
  // 防止旧页面的异步清理误伤随后新建的播放会话。
  if(player?.classList.contains('active')){
    const closingSessionId = playerSessionId;
    ++playerSessionId;
    void closeNativePlayback(closingSessionId);
  }
});
function sendPlayerCommand(command){
  // 只要 HTML5 视频层正在播放就直接驱动它，不依赖桌面运行时是否在位
  //（浏览器回退播放时进度条点击、倍速等同样要生效）。
  if(playerVideo && player.classList.contains('has-real-video')){
    const type = command?.type;
    if(type === 'togglePause') return togglePlay();
    if(type === 'play'){
      restorePlayerAudio();
      playerVideo.play().catch(error => toast('无法继续播放：' + backendErrorMessage(error)));
      return;
    }
    if(type === 'seek'){
      const position = Math.max(0, Math.min(TOTAL, Number(command.positionSeconds) || 0));
      playerVideo.currentTime = position;
      cur = playerVideo.currentTime;
      renderProgress();
      return;
    }
    if(type === 'setSpeed'){
      playerVideo.playbackRate = Number(command.speed) || 1;
      return;
    }
    if(type === 'setVolume'){
      const nextVolume = Math.max(0, Math.min(100, Number(command.volume) || 0));
      playerVideo.volume = nextVolume / 100;
      playerVideo.muted = nextVolume <= 0;
      return;
    }
    if(type === 'setSubtitleTrack'){
      const tracks = [...(playerVideo.textTracks || [])];
      const wanted = Number(command.trackId);
      tracks.forEach((track, index) => {
        track.mode = (Number.isFinite(wanted) ? index === wanted : track.mode === 'showing') ? 'showing' : 'disabled';
      });
      return;
    }
    if(type === 'setAudioTrack' && playerVideo.audioTracks){
      const wanted = Number(command.trackId);
      [...playerVideo.audioTracks].forEach((track, index) => {
        track.enabled = Number.isFinite(wanted) ? index === wanted : index === 0;
      });
      return;
    }
  }
  if(!isNativeMediaMode() || !selectedMovie?.playUrl) return;
  if(command?.type === 'seek'){
    pendingNativeSeek = Math.max(0, Math.min(TOTAL, Number(command.positionSeconds) || 0));
    pendingNativeSeekAt = Date.now();
  }
  TtvBackend.invoke('player_command', {command}).catch(error => toast('播放器命令未执行：' + backendErrorMessage(error)));
}

function activatePlayerShell(loading = false, options = {}){
  wasActive = currentView;
  document.querySelectorAll('.view').forEach(view => view.classList.remove('active'));
  player.classList.add('active');
  document.body.classList.add('player-active');
  toastEl?.classList.remove('show');
  if(!options.keepPicture){
    resetPlayerLoadLog();
    lastKnownVideoSize = {w:0, h:0};
    playerPreviewFrameCache.clear();
    seekPreviewBox()?.classList.remove('has-frame', 'is-waiting');
  }
  const keepPicture = Boolean(options.keepPicture) && (
    player.classList.contains('native-video-live') || player.classList.contains('has-real-video')
  );
  // 切集时保留上一帧，避免先黑屏再等 6.5 秒。全新打开仍回到不透明画布。
  if(!keepPicture){
    document.documentElement.classList.remove('native-video-live');
    player.classList.remove('native-video-live');
    nativeSwitchPending = false;
  }else{
    nativeSwitchPending = player.classList.contains('native-video-live');
  }
  nativeLoadWaitStartedAt = 0;
  nativePlaybackFallbackInFlight = false;
  nativeRevealArmedAt = 0;
  nativeFirstFrameStableAt = 0;
  // 上一个播放会话遗留的岛内弹窗（选集/更多/倍速/画质）不能带进新会话。
  closeAllIslandPopovers();
  history.replaceState(null, '', location.pathname + location.search + '#player');
  playing = false;
  setPlayIcon();
  setPlayerLoading(keepPicture ? false : loading, keepPicture ? '正在切换…' : undefined);
  renderPlayerVolume();
  uiActivity();
}

async function openPlayer(movie, sourceEl, skipExpand = false){
  const previousSession = playerSessionId;
  const sessionId = ++playerSessionId;
  browserVideoLoadId += 1;
  let activeMovie = movie || (currentView === 'home' ? homeMovieAt(current) : selectedMovie) || MOVIES[0];
  if(!activeMovie) return;
  const isEpisodeCollection = Array.isArray(activeMovie.episodes) && activeMovie.episodes.length > 0 && activeMovie.type !== 'episode';
  if(isStreamHubShow(activeMovie) || isEpisodeCollection){
    try{
      const episodes = await ensureMovieEpisodes(activeMovie);
      if(sessionId !== playerSessionId) return;
      if(!episodes.length){
        toast('此剧集没有绑定可播放的真实选集。');
        return;
      }
      activeMovie = createEpisodePlaybackMovie(activeMovie, episodes[0], 0);
    }catch(error){
      toast('无法读取真实剧集：' + backendErrorMessage(error));
      return;
    }
  }
  isCurrentInWatchlist = favoriteIds.has(String(activeMovie.id));
  syncPlayerContent(activeMovie);
  if(!isNativeMediaMode() && !activeMovie.playUrl){
    toast('当前是公开目录元数据，没有绑定真实视频文件。');
    return;
  }
  if(isNativeMediaMode() && !activeMovie.forceWebPlayback){
    const keepPicture = player.classList.contains('active') && (
      player.classList.contains('native-video-live') || player.classList.contains('has-real-video')
    );
    activatePlayerShell(true, {keepPicture});
    if(!isPlayerSessionActive(sessionId)) return;
    try{
      const historyPromise = TtvBackend.invoke('history_get', {mediaId: String(activeMovie.id)}).catch(() => null);
      if(activeMovie.providerId){
        setPlayerLoading(true, '正在连接…');
        const providerMediaId = activeMovie.providerMediaId || (
          String(activeMovie.id || '').startsWith('provider:streamhub:')
            ? String(activeMovie.id).slice('provider:streamhub:'.length)
            : ''
        );
        if(!providerMediaId){
          toast('此条目缺少来源媒体 ID，请重新从 StreamHub 同步。');
          return;
        }
        const playback = activeMovie.providerId === 'openlist'
          ? await TtvBackend.invoke('openlist_resolve_playback', {input:{storageId:String(activeMovie.openlistStorageId || activeMovie.storageId || ''), path:String(activeMovie.openlistPath || providerMediaId), mediaId:String(activeMovie.id), quality:activeMovie.playbackQualityGcid || activeMovie.playbackQuality || null}})
          : await TtvBackend.invoke('provider_resolve_playback', {
            providerId: activeMovie.providerId,
            request: {mediaId: providerMediaId, quality: activeMovie.playbackQualityGcid || activeMovie.playbackQuality || null}
          });
        if(!isPlayerSessionActive(sessionId)) return;
        activeMovie.playUrl = playback.url;
        activeMovie.playHeaders = playback.headers || {};
        activeMovie.playbackExpiresAt = playback.expiresAt ?? playback.expires_at ?? null;
        // 光鸭等云盘来源会带回完整清晰度表(videoResource[])与当前选中的画质名,
        // 画质菜单/弹窗据此提供真实的分辨率切换。
        activeMovie.qualities = Array.isArray(playback.qualities) ? playback.qualities : null;
        if(playback.quality) activeMovie.playbackQuality = String(playback.quality);
      }
      if(!activeMovie.playUrl){
        toast('此条目没有可播放文件，请先扫描媒体目录或检查来源连接。');
        return;
      }
      // Signed cloud URLs are short-lived. Re-resolve before probing or
      // opening a player so stale links do not masquerade as codec failures.
      if(activeMovie.providerId && Number(activeMovie.playbackExpiresAt) > 0 &&
         Number(activeMovie.playbackExpiresAt) <= Math.floor(Date.now() / 1000) + 15){
        const providerMediaId = activeMovie.providerMediaId || (
          String(activeMovie.id || '').startsWith('provider:streamhub:')
            ? String(activeMovie.id).slice('provider:streamhub:'.length)
            : ''
        );
        const refreshed = activeMovie.providerId === 'openlist'
          ? await TtvBackend.invoke('openlist_resolve_playback', {input:{storageId:String(activeMovie.openlistStorageId || activeMovie.storageId || ''), path:String(activeMovie.openlistPath || providerMediaId), mediaId:String(activeMovie.id), quality:activeMovie.playbackQualityGcid || activeMovie.playbackQuality || null}})
          : await TtvBackend.invoke('provider_resolve_playback', {providerId: activeMovie.providerId, request: {mediaId: providerMediaId, quality: activeMovie.playbackQualityGcid || activeMovie.playbackQuality || null}});
        if(!isPlayerSessionActive(sessionId)) return;
        activeMovie.playUrl = refreshed.url;
        activeMovie.playHeaders = refreshed.headers || {};
        activeMovie.playbackExpiresAt = refreshed.expiresAt ?? refreshed.expires_at ?? null;
      }
      // 原生播放器可直接读取远程媒体。不要在首帧前同步等待 FFprobe：
      // 大文件或远端尾部元数据会让探测耗时远高于真正开始播放的时间。
      let mediaProbe = activeMovie.mediaProbe || null;
      let nativeOpened = false;
      const wantNativeInterp = enhancementRifeEnabled || !!activeMovie.forceNativePlayback;
      if(movieCanPlayInBrowser(activeMovie) && !wantNativeInterp){
        try{
          setPlayerLoading(keepPicture ? false : true, '正在打开…');
          const browserMovie = activeMovie.browserPlayUrl
            ? {...activeMovie, playUrl: activeMovie.browserPlayUrl}
            : activeMovie;
          const browserLoadedFast = await loadBrowserVideo(browserMovie);
          if(discardStalePlayerSession(sessionId)) return;
          if(browserLoadedFast){
            document.documentElement.classList.remove('native-video-live');
            player.classList.remove('native-video-live');
            nativeSwitchPending = false;
            void closeNativePlayback(previousSession);
            nativeLoadWaitStartedAt = 0;
            await restoreWatchPosition(activeMovie);
            setPlayerLoading(false);
            startTimer();
            uiActivity();
            return;
          }
        }catch(error){
          console.warn('Fast HTML5 path unavailable, continuing with native:', error);
        }
      }
      try{
        setPlayerLoading(true, keepPicture ? '正在切换…' : '正在启动播放器…');
        resetPlayerVideo();
        const history = await historyPromise;
        pushPlayerLoadLog(isLocalMediaSource(activeMovie.playUrl) ? '正在打开本地文件…' : '正在连接媒体地址…');
        nativeOpened = await TtvBackend.invoke('player_native_open', {
          input: {
            url: activeMovie.playUrl,
            mediaId: String(activeMovie.id),
            headers: activeMovie.playHeaders || {},
            decryptionKey: activeMovie.decryptionKey || undefined,
            resumePositionSeconds: history && !history.completed ? Number(history.positionSeconds) || 0 : 0,
            media: mediaProbe || undefined,
            audioTrack: Number.isFinite(Number(activeMovie.audioTrack)) ? Number(activeMovie.audioTrack) : undefined,
            subtitleTrack: Number.isFinite(Number(activeMovie.subtitleTrack)) ? Number(activeMovie.subtitleTrack) : undefined,
            preferredAudioLanguage: activeMovie.preferredAudioLanguage || navigator.language?.split('-')[0] || undefined,
            preferredSubtitleLanguage: activeMovie.preferredSubtitleLanguage || navigator.language?.split('-')[0] || undefined,
            interpolation: activeMovie.interpolation,
            hdr: activeMovie.hdr,
            sessionId
          }
        });
        if(!isPlayerSessionActive(sessionId)){
          // Close the actor created by this stale open request. Passing its
          // original token is intentional: a newer session, if any, is
          // protected by the backend token check and will not be torn down.
          if(nativeOpened) await closeNativePlayback(sessionId);
          return;
        }
        if(nativeOpened){
          const externalSubtitles = Array.isArray(mediaProbe?.externalSubtitles) ? mediaProbe.externalSubtitles : [];
          if(externalSubtitles[0]){
            void TtvBackend.invoke('subtitle_attach', {input: {path: externalSubtitles[0], select: true}}).catch(error => {
              console.warn('Unable to attach detected external subtitle:', error);
            });
          }
          // 本地文件的音轨/外挂字幕信息在播放启动后补齐，不再阻塞首帧。
          if(!mediaProbe && isLocalMediaSource(activeMovie.playUrl)){
            void TtvBackend.invoke('media_probe', {
              input: {url: activeMovie.playUrl, headers: activeMovie.playHeaders || {}}
            }).then(async probe => {
              mediaProbe = probe;
              activeMovie.mediaProbe = probe;
              if(Number(probe?.durationSeconds) > 0) activeMovie.durationSeconds = Number(probe.durationSeconds);
              const subtitle = Array.isArray(probe?.externalSubtitles) ? probe.externalSubtitles[0] : null;
              if(subtitle){
                await TtvBackend.invoke('subtitle_attach', {input: {path: subtitle, select: true}});
              }
            }).catch(error => console.warn('Deferred local media probe unavailable:', error));
          }
          const enhancementMode = activeMovie.interpolationMode ?? null;
          if(enhancementMode !== null){
            void TtvBackend.invoke('enhancement_apply', {mode: Number(enhancementMode) || 0}).catch(error => {
              console.warn('Unable to apply playback enhancement:', error);
            });
          }
          const sourceText = document.getElementById('pSource');
          if(sourceText) sourceText.textContent = (activeMovie.sourceLabel || '本地媒体') + ' · 原生 libmpv 播放窗口';
        }
      }catch(error){
        console.warn('Native libmpv window unavailable; using embedded fallback:', error);
      }
      let browserLoaded = false;
      try{
      if(!isPlayerSessionActive(sessionId)) return;
      if(!nativeOpened && activeMovie.decryptionKey){
        // CENC 加密直链只有 libmpv（decryption_key）能解，浏览器层必然失败，
        // 直接给出明确错误，避免落入慢速转码兜底。
        throw new Error('加密直链需要原生播放器（libmpv），当前环境不可用。');
      }
      if(nativeOpened){
          // mpv 已嵌入主窗口 WebView2 底下。先保持不透明加载画面并启动状态同步
          // （进度/时长/播放图标都靠它），等 syncBackendPlayback 确认文件加载完成
          // 再把画布变透明让视频透出，避免首帧渲染前出现透明窗口。
          setPlayerLoading(true, keepPicture ? '正在切换画面…' : '正在解码首帧…');
          nativeSwitchPending = keepPicture || !player.classList.contains('native-video-live');
          nativePlayNudgeAt = 0;
          if(history && !history.completed){
            TOTAL = Math.max(1, Number(history.durationSeconds) || Number(activeMovie.durationSeconds) || TOTAL);
            const saved = Math.min(TOTAL, Math.max(0, Number(history.positionSeconds) || 0));
            cur = (TOTAL > 0 && saved / TOTAL >= 0.95) ? 0 : saved;
            renderProgress();
          }
          sendPlayerCommand({type:'play'});
          sendPlayerCommand({type:'setVolume', volume: playerMuted ? 0 : playerVolume});
          nativeLoadWaitStartedAt = Date.now();
          nativePlaybackFallbackInFlight = false;
          nativeRevealArmedAt = 0;
          startTimer();
          restoreEnhancementUi();
          uiActivity();
          return;
        }
        setPlayerLoading(true, '正在打开…');
        const browserMovie = activeMovie.browserPlayUrl
          ? {...activeMovie, playUrl: activeMovie.browserPlayUrl}
          : activeMovie;
        browserLoaded = await loadBrowserVideo(browserMovie);
        if(discardStalePlayerSession(sessionId)) return;
      }catch(error){
        console.warn('HTML5 video layer unavailable, falling back to libmpv:', error);
      }
      if(!browserLoaded && !movieRequiresNativeDecoder(activeMovie) && !isHongguoPlaybackId(activeMovie.id)
         && (isLocalMediaSource(activeMovie.playUrl) || /^https?:\/\//i.test(String(activeMovie.playUrl || '')))){
        try{
          setPlayerLoading(true, '正在适配格式…');
          const prepared = await withTimeout(TtvBackend.invoke('player_prepare_browser_media', {
            input: {url: activeMovie.playUrl, headers: activeMovie.playHeaders || {}}
          }), 12000, '兼容转码超时');
          if(prepared?.url){
            browserLoaded = await loadBrowserVideo({...activeMovie, playUrl: prepared.url});
            if(discardStalePlayerSession(sessionId)) return;
            if(browserLoaded) activeMovie.browserPlayUrl = prepared.url;
            if(browserLoaded) toast(prepared.cached ? '已使用播放缓存继续播放' : '已将当前格式转换为兼容播放流');
          }
        }catch(error){
          console.warn('Browser-compatible playback preparation failed:', error);
        }
      }
      if(!browserLoaded){
        // A headless actor is useful for diagnostics, but it is not a visible
        // user-facing fallback. Try an installed external player instead and
        // surface a clear error when none is available.
        const externalPlayers = await TtvBackend.invoke('external_player_list').catch(() => []);
        if(!isPlayerSessionActive(sessionId)) return;
        const external = Array.isArray(externalPlayers) ? externalPlayers[0] : null;
        if(!external?.id){
          throw new Error('原生窗口、浏览器兼容流和 FFmpeg 均不可用；未检测到外部播放器');
        }
        if(!isPlayerSessionActive(sessionId)) return;
        await TtvBackend.invoke('external_player_open', {input:{
          playerId: external.id,
          url: activeMovie.playUrl,
          headers: activeMovie.playHeaders || {}
        }});
        if(!isPlayerSessionActive(sessionId)) return;
        toast(`已交由 ${external.id} 播放`);
      }
      if(browserLoaded){
        document.documentElement.classList.remove('native-video-live');
        player.classList.remove('native-video-live');
        nativeSwitchPending = false;
        void closeNativePlayback(previousSession);
      }
      await restoreWatchPosition(activeMovie);
      setPlayerLoading(false);
      uiActivity();
      return;
    }catch(error){
      setPlayerLoading(false);
      playing = false;
      setPlayIcon();
      const sourceText = document.getElementById('pSource');
      if(sourceText) sourceText.textContent = '播放失败 · ' + backendErrorMessage(error);
      toast('无法加载此文件：' + backendErrorMessage(error));
      return;
    }
  }
  const activateDirect = () => {
    if(sessionId !== playerSessionId) return;
    activatePlayerShell(false);
  };

  if(skipExpand || reducedMotion || isPlayerExpanding){
    activateDirect();
    return;
  }

  isPlayerExpanding = true;

  // 首页专享：从中央视口黄金画幅向四周平滑拉伸至全屏；其他所有区域：根据点击卡片的物理位置向全屏扩张
  let startRect = null;
  let startRadius = '24px';

  if(currentView === 'home'){
    const vw = window.innerWidth, vh = window.innerHeight;
    startRect = {
      top: vh * 0.21,
      left: vw * 0.28,
      width: vw * 0.44,
      height: vh * 0.58
    };
    startRadius = '24px';
  } else if(sourceEl && typeof sourceEl.getBoundingClientRect === 'function'){
    const imgEl = sourceEl.querySelector('img') || sourceEl;
    startRect = imgEl.getBoundingClientRect();
    try {
      const style = window.getComputedStyle(sourceEl);
      if(style && style.borderRadius) startRadius = style.borderRadius;
    } catch(e){}
  }

  // 兜底：若未获取到具体坐标，则从中央区域起步
  if(!startRect || startRect.width < 10 || startRect.height < 10){
    const vw = window.innerWidth, vh = window.innerHeight;
    startRect = {
      top: vh * 0.22,
      left: vw * 0.28,
      width: vw * 0.44,
      height: vh * 0.56
    };
  }

  const stage = document.createElement('div');
  stage.className = 'scroll-expand-stage';

  const frame = document.createElement('div');
  frame.className = 'scroll-expand-frame';
  frame.style.top = startRect.top + 'px';
  frame.style.left = startRect.left + 'px';
  frame.style.width = startRect.width + 'px';
  frame.style.height = startRect.height + 'px';
  frame.style.borderRadius = startRadius;

  frame.innerHTML = `
    <img class="scroll-expand-media" src="${activeMovie.img}" alt="${activeMovie.t}" />
    <div class="scroll-expand-scrim"></div>
    <div class="scroll-expand-title">${activeMovie.t}</div>
    <div class="scroll-expand-overlay">
      <div class="se-loading-ring"></div>
      <div class="se-play-tag">正在打开…</div>
    </div>
  `;
  stage.appendChild(frame);
  document.body.appendChild(stage);

  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      frame.classList.add('expanded');

      setTimeout(() => {
        activateDirect();
        setTimeout(() => {
          stage.style.opacity = '0';
          setTimeout(() => {
            stage.remove();
            isPlayerExpanding = false;
          }, 350);
        }, 120);
      }, 680);
    });
  });
}
function closePlayer(){
  if(document.documentElement.classList.contains('is-window-pip') || isPipActive){
    void restoreFromPip();
    return;
  }
  if(document.documentElement.classList.contains('is-os-fullscreen') && TtvBackend.available()){
    void TtvBackend.invoke('app_window_toggle_fullscreen').catch(() => {});
  }
  applyWindowChromeClasses({fullscreen: false, pip: false});
  isPipActive = false;
  const closingSessionId = playerSessionId;
  ++playerSessionId;
  browserVideoLoadId += 1;
  clearShortDramaNextCountdown();
  shortDramaPreparedNext = null;
  clearShortDramaBuffer();
  saveWatchProgress();
  if(watchProgressTimer){ clearTimeout(watchProgressTimer); watchProgressTimer = null; }
  nativeLoadWaitStartedAt = 0;
  nativePlaybackFallbackInFlight = false;
  nativeSwitchPending = false;
  nativeRevealArmedAt = 0;
  nativeFirstFrameStableAt = 0;
  // 结束单窗口嵌入播放：恢复画布不透明（mpv 表面由 player_native_close 销毁）
  document.documentElement.classList.remove('native-video-live');
  if(player) player.classList.remove('native-video-live');
  if(playerVideo && player.classList.contains('has-real-video')) resetPlayerVideo();
  if(TtvBackend.available()) closeNativePlayback(closingSessionId);
  else if(!(playerVideo && player.classList.contains('has-real-video'))) sendPlayerCommand({type: 'unload'});
  if(isPlayerLocked) togglePlayerLock();
  player.classList.remove('active');
  document.body.classList.remove('player-active');
  stopTimer();
  stopDanmaku();
  const back = document.getElementById('view-' + wasActive) ? wasActive : 'home';
  currentView = '__none__';
  showView(back);
}
function togglePlay(){
  if(playerVideo && player.classList.contains('has-real-video')){
    restorePlayerAudio();
    if(playerVideo.paused){ playerVideo.play().catch(error => toast('无法继续播放：' + backendErrorMessage(error))); }
    else playerVideo.pause();
    playing = !playerVideo.paused;
    setPlayIcon();
    playing ? startTimer() : stopTimer();
    uiActivity();
    return;
  }
  playing = !playing;
  sendPlayerCommand({type: 'togglePause'});
  setPlayIcon();
  playing ? startTimer() : stopTimer();
  uiActivity();
}
function setPlayIcon(){
  playIcon.innerHTML = playing
    ? '<path d="M7 5h4v14H7zM13 5h4v14h-4z"/>'
    : '<path d="M8 5.5v13l11-6.5z"/>';
}
function startTimer(){
  stopTimer();
  // 原生画布尚未变透明（首帧未透出）时用 250ms 快速同步，让出画揭开更跟手；
  // 变透明后回到 1s 常规心跳。切集复用 actor 时同样快轮询直到新首帧。
  const fastPoll = isNativeMediaMode() && (!player.classList.contains('native-video-live') || nativeSwitchPending);
  timer = setInterval(() => {
    if(isNativeMediaMode()) syncBackendPlayback();
    else { cur = (cur + 1) % TOTAL; renderProgress(); }
  }, fastPoll ? 250 : 1000);
}
function stopTimer(){ if(timer){ clearInterval(timer); timer = null; } }
function seek(d){
  if(playerVideo && player.classList.contains('has-real-video')){
    playerVideo.currentTime = Math.max(0, Math.min(TOTAL, playerVideo.currentTime + d));
    cur = playerVideo.currentTime;
    renderProgress();
    uiActivity();
    maybeArmShortDramaTailCountdown();
    return;
  }
  if(!(TOTAL > 0)) return; // 时长未知时不跳转，避免误跳回开头
  cur = Math.max(0, Math.min(TOTAL, cur + d));
  sendPlayerCommand({type: 'seek', positionSeconds: cur});
  maybeArmShortDramaTailCountdown();
  renderProgress();
  uiActivity();
}
const progressBar = document.getElementById('pBar');
let isProgressDragging = false;
let progressDragMoved = false;
let suppressProgressClickUntil = 0;
function seekToPointer(clientX){
  if(!(TOTAL > 0) || !progressBar) return;
  const rect = progressBar.getBoundingClientRect();
  if(!(rect.width > 0)) return;
  const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
  cur = ratio * TOTAL;
  renderProgress();
}
function commitProgressSeek(){
  if(!(TOTAL > 0)) return;
  if(playerVideo && player.classList.contains('has-real-video')){
    playerVideo.currentTime = cur;
  }else{
    sendPlayerCommand({type:'seek', positionSeconds:cur});
  }
  maybeArmShortDramaTailCountdown();
}
progressBar?.addEventListener('pointerdown', event => {
  if(!(TOTAL > 0) || (event.pointerType === 'mouse' && event.button !== 0)) return;
  isProgressDragging = true;
  progressDragMoved = false;
  progressBar.classList.add('dragging');
  progressBar.setPointerCapture?.(event.pointerId);
  seekToPointer(event.clientX);
  uiActivity();
  event.preventDefault();
});
progressBar?.addEventListener('pointermove', event => {
  showPlayerSeekPreview(event.clientX);
  if(isProgressDragging){
    progressDragMoved = true;
    seekToPointer(event.clientX);
    uiActivity();
  }
});
progressBar?.addEventListener('pointerenter', event => showPlayerSeekPreview(event.clientX));
progressBar?.addEventListener('pointerleave', () => { if(!isProgressDragging) hidePlayerSeekPreview(); });
function finishProgressDrag(event){
  if(!isProgressDragging) return;
  commitProgressSeek();
  if(progressDragMoved) suppressProgressClickUntil = Date.now() + 250;
  isProgressDragging = false;
  progressBar?.classList.remove('dragging');
  hidePlayerSeekPreview();
  if(event?.pointerId !== undefined && progressBar?.hasPointerCapture?.(event.pointerId)){
    progressBar.releasePointerCapture(event.pointerId);
  }
  uiActivity();
}
progressBar?.addEventListener('pointerup', finishProgressDrag);
progressBar?.addEventListener('pointercancel', finishProgressDrag);
window.addEventListener('pointerup', finishProgressDrag);
progressBar?.addEventListener('click', event => {
  if(Date.now() < suppressProgressClickUntil) return;
  if(!(TOTAL > 0)) return;
  seekToPointer(event.clientX);
  commitProgressSeek();
  uiActivity();
});
progressBar?.addEventListener('lostpointercapture', () => {
  if(isProgressDragging) commitProgressSeek();
  isProgressDragging = false;
  progressBar.classList.remove('dragging');
  hidePlayerSeekPreview();
});
function uiActivity(){
  if(isPlayerLocked) return;
  playerUi.classList.remove('hide-ui');
  if(hideTimer) clearTimeout(hideTimer);
  hideTimer = setTimeout(() => {
    if(!isPlayerLocked && !isProgressDragging && !hasOpenPlayerPopover()) playerUi.classList.add('hide-ui');
  }, 3400);
}
player.addEventListener('mousemove', uiActivity);
player.addEventListener('pointermove', uiActivity);
player.addEventListener('touchstart', uiActivity, {passive:true});
player.addEventListener('wheel', uiActivity, {passive:true});
player.addEventListener('click', e => { if((e.target === player || e.target.classList.contains('player-bg') || e.target.classList.contains('player-shade')) && !isPlayerLocked) uiActivity(); });

/* 屏幕锁定 / 解锁：隐藏所有控制组件与恢复 */
let isPlayerLocked = false;
function togglePlayerLock(e){
  e?.stopPropagation?.();
  e?.preventDefault?.();
  isPlayerLocked = !isPlayerLocked;
  const playerEl = document.getElementById('view-player');
  const lockBtn = document.getElementById('playerLockBtn');
  const playerUiEl = document.getElementById('playerUi');
  if(!playerEl || !lockBtn) return;

  if(isPlayerLocked){
    playerEl.classList.add('player-locked');
    if(playerUiEl) playerUiEl.classList.add('hide-ui');
    lockBtn.classList.add('locked');
    lockBtn.title = '解锁屏幕 (点击恢复所有控件)';
    lockBtn.innerHTML = `
      <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2">
        <rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect>
        <path d="M7 11V7a5 5 0 0 1 10 0v4"></path>
      </svg>
    `;
    closeAllIslandPopovers();
    toast('🔒 屏幕已锁定，所有控件已隐藏');
  } else {
    playerEl.classList.remove('player-locked');
    if(playerUiEl) playerUiEl.classList.remove('hide-ui');
    lockBtn.classList.remove('locked');
    lockBtn.title = '锁定屏幕 (隐藏所有控件)';
    lockBtn.innerHTML = `
      <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2">
        <rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect>
        <path d="M7 11V7a5 5 0 0 1 9.9-1"></path>
      </svg>
    `;
    uiActivity();
    toast('🔓 屏幕已解锁，控件已恢复');
  }
}

function applyWindowChromeClasses({fullscreen = false, pip = false} = {}){
  document.documentElement.classList.toggle('is-os-fullscreen', Boolean(fullscreen));
  document.documentElement.classList.toggle('is-window-pip', Boolean(pip));
  document.body?.classList.toggle('is-window-pip', Boolean(pip));
  document.getElementById('btnFullscreen')?.classList.toggle('on', Boolean(fullscreen));
  document.getElementById('btnPip')?.classList.toggle('on', Boolean(pip));
  if(pip){
    closeAllIslandPopovers?.();
    document.getElementById('playerVolumeControl')?.classList.remove('active-hover', 'dragging');
    document.getElementById('volumePopover')?.classList.remove('show');
    document.getElementById('playerVolumeTrack')?.classList.remove('dragging');
  }
}

/* 全屏切换：桌面端铺满当前显示器（含任务栏），浏览器再回退到页面全屏。 */
async function toggleFullscreen(){
  try{
    if(TtvBackend.available()){
      const isFullscreen = await TtvBackend.invoke('app_window_toggle_fullscreen');
      applyWindowChromeClasses({fullscreen: Boolean(isFullscreen), pip: false});
      return;
    }
    const currentWindow = window.__TAURI__?.window?.getCurrentWindow?.();
    if(currentWindow?.isFullscreen && currentWindow?.setFullscreen){
      const isFullscreen = await currentWindow.isFullscreen();
      await currentWindow.setFullscreen(!isFullscreen);
      applyWindowChromeClasses({fullscreen: !isFullscreen, pip: false});
      return;
    }
  }catch(error){
    toast('窗口全屏切换失败：' + backendErrorMessage(error));
    return;
  }
  try{
    if(!document.fullscreenElement){
      await document.documentElement.requestFullscreen();
      applyWindowChromeClasses({fullscreen: true, pip: false});
    }else{
      await document.exitFullscreen();
      applyWindowChromeClasses({fullscreen: false, pip: false});
    }
  }catch(error){
    toast('当前页面无法切换全屏：' + backendErrorMessage(error));
  }
}

/* 播放器窗口置顶切换 */
let isPlayerWindowPinned = false;
function togglePinPlayerWindow(btn){
  isPlayerWindowPinned = !isPlayerWindowPinned;
  if(btn) btn.classList.toggle('active', isPlayerWindowPinned);
  if(TtvBackend.available()){
    TtvBackend.invoke('player_command', {command:{type:'setAlwaysOnTop', enabled:isPlayerWindowPinned}})
      .then(() => TtvBackend.invoke('settings_set', {key: 'window.alwaysOnTop', value: isPlayerWindowPinned ? '1' : '0'}))
      .catch(e => {
        isPlayerWindowPinned = !isPlayerWindowPinned;
        if(btn) btn.classList.toggle('active', isPlayerWindowPinned);
        toast('窗口置顶未执行：' + backendErrorMessage(e));
      });
  }else{
    toast('当前页面未连接桌面窗口，无法置顶播放器。');
    return;
  }
  toast(isPlayerWindowPinned ? '播放窗口已置顶' : '已取消播放窗口置顶');
}

/* 帧率监控只展示当前视频真实采样，不切换虚构的输出档位。 */
function cycleFpsMode(){
  toggleFpsPanel();
}

/* 帧率面板开关 */
function toggleFpsPanel(){
  const p = document.getElementById('fpsPanel');
  const chip = document.getElementById('chipFps');
  if(p){
    const isHidden = p.style.display === 'none';
    p.style.display = isHidden ? 'block' : 'none';
    if(chip) chip.classList.toggle('on', isHidden);
    toast(isHidden ? '实时帧率面板已开启' : '实时帧率面板已隐藏');
  }
}

/* 倍速切换 */
const SPEEDS = ['1.0x', '1.25x', '1.5x', '2.0x', '0.75x'];
let speedIdx = 0;
function applyPlaybackSpeed(speed, btn = document.getElementById('chipSpeed'), notify = true){
  const requested = Math.max(0.25, Math.min(4, Number(speed) || 1));
  let closestIdx = 0;
  let closestDistance = Infinity;
  SPEEDS.forEach((label, index) => {
    const distance = Math.abs(Number.parseFloat(label) - requested);
    if(distance < closestDistance){
      closestDistance = distance;
      closestIdx = index;
    }
  });
  speedIdx = closestIdx;
  const label = btn?.querySelector('.chip-label');
  if(label) label.textContent = SPEEDS[speedIdx];
  const value = Number.parseFloat(SPEEDS[speedIdx]);
  sendPlayerCommand({type: 'setSpeed', speed: value});
  if(notify) toast('当前播放倍速: ' + SPEEDS[speedIdx]);
}
function cyclePlaybackSpeed(btn){
  applyPlaybackSpeed(Number.parseFloat(SPEEDS[(speedIdx + 1) % SPEEDS.length]), btn);
}
function changePlaybackSpeed(delta){
  const direction = Number(delta) >= 0 ? 1 : -1;
  speedIdx = (speedIdx + direction + SPEEDS.length) % SPEEDS.length;
  const btn = document.getElementById('chipSpeed');
  const label = btn?.querySelector('.chip-label');
  if(label) label.textContent = SPEEDS[speedIdx];
  sendPlayerCommand({type: 'setSpeed', speed: Number.parseFloat(SPEEDS[speedIdx])});
  toast('当前播放倍速: ' + SPEEDS[speedIdx]);
}

/* 跳过片头片尾 */
function skipIntro(){
  const intro = Number(selectedMovie?.metadata?.introSeconds);
  if(!Number.isFinite(intro) || intro <= 0){
    toast('当前媒体没有提供可跳过片头时长。');
    return;
  }
  seek(intro);
  toast('已跳过媒体报告的片头时长。');
}

/* 上一集 / 下一集 */
function playPrevEpisode(){
  playAdjacentEpisode(-1);
}
function playNextEpisode(){
  playAdjacentEpisode(1);
}
function playAdjacentEpisode(direction){
  // 短剧 / 漫剧会话：用 vid 列表换集（锁定集自动云端解析）。
  if(isHongguoPlaybackId(selectedMovie?.id) && shortDramaCtx){
    const currentIndex = Number.isInteger(shortDramaCtx.currentIndex)
      ? shortDramaCtx.currentIndex
      : Math.max(0, Number(shortDramaCtx.episode) - 1);
    const nextIndex = currentIndex + direction;
    if(nextIndex < 0) return toast('已经是第一集。');
    const nextVid = shortDramaCtx.vids[nextIndex];
    if(!nextVid) return toast('已经是最后一集。');
    void playShortDramaEpisode(shortDramaCtx.seriesId, nextVid, shortDramaCtx.detail, null);
    return;
  }
  const parent = selectedMovie?.episodes?.length ? selectedMovie : detailMovie;
  const episodes = Array.isArray(parent?.episodes) ? parent.episodes : [];
  const currentIndex = activeEpisodeIndex(episodes);
  if(!episodes.length || currentIndex < 0){
    toast('当前播放内容没有可切换的真实选集。');
    return;
  }
  const nextIndex = currentIndex + direction;
  if(nextIndex < 0 || nextIndex >= episodes.length){
    toast(direction < 0 ? '已经是第一集。' : '已经是最后一集。');
    return;
  }
  playEpisode(nextIndex, null, parent);
}

/* 播放器追剧切换 */
let isCurrentInWatchlist = false;
function togglePlayerWatchlist(btn){
  isCurrentInWatchlist = !isCurrentInWatchlist;
  if(btn) btn.classList.toggle('on', isCurrentInWatchlist);
  if(selectedMovie?.id) persistFavorite(selectedMovie, isCurrentInWatchlist);
  toast(isCurrentInWatchlist ? '💖 已将本剧加入追剧收藏夹' : '已从追剧收藏夹移出');
}

/* 画中画：原生 libmpv 没有 HTML5 画面，走置顶小窗；浏览器直链才用 document PiP。 */
let isPipActive = false;
function pipVideoSize(){
  applyKnownPreviewAspect();
  const w = Number(lastKnownVideoSize.w) || Number(playerVideo?.videoWidth) || 0;
  const h = Number(lastKnownVideoSize.h) || Number(playerVideo?.videoHeight) || 0;
  return {videoWidth: w > 0 ? w : undefined, videoHeight: h > 0 ? h : undefined};
}
async function restoreFromPip(){
  if(!isPipActive && !document.documentElement.classList.contains('is-window-pip')) return false;
  if(document.pictureInPictureElement){
    try{ await document.exitPictureInPicture(); }catch(error){ /* ignore */ }
  }else if(TtvBackend.available()){
    try{ await TtvBackend.invoke('app_window_toggle_pip'); }catch(error){ /* ignore */ }
  }
  isPipActive = false;
  applyWindowChromeClasses({fullscreen: false, pip: false});
  return true;
}
async function togglePipMode(){
  const html5Ready = Boolean(
    playerVideo
    && player?.classList.contains('has-real-video')
    && !player?.classList.contains('native-video-live')
    && playerVideo.readyState >= 2
    && document.pictureInPictureEnabled
  );
  if(html5Ready){
    try{
      if(document.pictureInPictureElement){
        await document.exitPictureInPicture();
        isPipActive = false;
      }else{
        await playerVideo.requestPictureInPicture();
        isPipActive = true;
      }
      applyWindowChromeClasses({fullscreen: false, pip: isPipActive});
      return;
    }catch(error){
      console.warn('HTML5 picture-in-picture unavailable:', error);
    }
  }
  if(!TtvBackend.available()){
    toast('当前播放引擎不支持画中画');
    return;
  }
  try{
    const pip = await TtvBackend.invoke('app_window_toggle_pip', {input: pipVideoSize()});
    isPipActive = Boolean(pip);
    applyWindowChromeClasses({fullscreen: false, pip: isPipActive});
  }catch(error){
    toast('画中画未开启：' + backendErrorMessage(error));
  }
}

/* 选集面板：与底部控制栏的剧集面板共用紧凑锚定式样，不再遮罩播放画面。 */
function openEpisodePickerModal(source){
  // 短剧 / 漫剧会话：列出全部集数，统一使用可点击样式。
  if(isHongguoPlaybackId(selectedMovie?.id) && shortDramaCtx){
    const context = shortDramaCtx;
    const comic = context.kind === 'comic';
    const anchor = resolvePlayerPopoverAnchor(source, 'btnEpisode');
    const chips = context.vids.map((vid, index) => {
      return `<button class="sd-picker-chip${index + 1 === context.episode ? ' active' : ''}" data-sd-index="${index}">${index + 1}</button>`;
    }).join('');
    openPlayerActionPopover({
      kind: 'shortdrama-episodes',
      title: comic ? '漫剧选集' : '短剧选集',
      hint: `共 ${context.vids.length} 集 · 点击任意集数切换播放`,
      anchor,
      variant: 'wide',
      bodyHtml: `<div class="sd-picker-grid">${chips}</div>`
    });
    document.querySelectorAll('.sd-picker-chip').forEach(chip => {
      chip.addEventListener('click', () => {
        const index = Number(chip.dataset.sdIndex);
        const vid = context.vids[index];
        if(!vid) return;
        closePlayerActionPopover(true);
        void playShortDramaEpisode(context.seriesId, vid, context.detail, null);
      });
    });
    return;
  }
  const parent = selectedMovie?.episodes?.length ? selectedMovie : detailMovie;
  const episodes = Array.isArray(parent?.episodes) ? parent.episodes : [];
  if(!episodes.length){
    toast('当前内容没有真实选集数据。');
    return;
  }
  const activeIndex = activeEpisodeIndex(episodes);
  const anchor = resolvePlayerPopoverAnchor(source, 'btnEpisode');
  openPlayerActionPopover({
    kind: 'episodes',
    title: '剧集选择',
    hint: activeIndex >= 0 ? `当前第 ${activeIndex + 1} 集 · 共 ${episodes.length} 集` : `共 ${episodes.length} 集`,
    anchor,
    variant: 'wide',
    bodyHtml: `<div class="player-popover-episode-list">
      ${episodes.map((item, index) => `
        <button class="player-popover-item player-popover-episode${index === activeIndex ? ' active' : ''}" onclick="playEpisodeFromPicker(${index})">
          <span><b>${escapeHtml(episodeShortLabel(item) + (item.title ? ' · ' + item.title : ''))}</b><small>${escapeHtml(item.durationLabel || formatDuration(item.durationSeconds))}</small></span>
          ${index === activeIndex ? '<em>正在播放</em>' : `<em>${String(index + 1).padStart(2, '0')}</em>`}
        </button>
      `).join('')}
    </div>`
  });
}
function playEpisodeFromPicker(episodeIndex){
  const parent = selectedMovie?.episodes?.length ? selectedMovie : detailMovie;
  closePlayerActionPopover();
  closeModal();
  playEpisode(episodeIndex, null, parent);
}

function setSubtitleModeLabel(label){
  const target = document.getElementById('pSubtitleModeText');
  if(target) target.textContent = label || '字幕列表';
}
function srtToVtt(text){
  const body = String(text || '').replace(/\r/g, '').replace(/(\d+:\d+:\d+),(\d+)/g, '$1.$2');
  return body.startsWith('WEBVTT') ? body : 'WEBVTT\n\n' + body;
}
function attachHtmlSubtitle(content, label){
  if(!playerVideo) return false;
  [...playerVideo.querySelectorAll('track[data-ttv-sub]')].forEach(track => track.remove());
  const blob = new Blob([srtToVtt(content)], {type: 'text/vtt'});
  const url = URL.createObjectURL(blob);
  const track = document.createElement('track');
  track.kind = 'subtitles';
  track.label = label || '外挂字幕';
  track.srclang = 'zh';
  track.default = true;
  track.dataset.ttvSub = '1';
  track.src = url;
  playerVideo.appendChild(track);
  const apply = () => {
    [...(playerVideo.textTracks || [])].forEach(item => {
      item.mode = item.label === track.label ? 'showing' : 'disabled';
    });
  };
  track.addEventListener('load', apply);
  setTimeout(apply, 80);
  return true;
}

/* 语言码 → 中文名(官方播放器同款思路,覆盖常见字幕/音轨语言标记) */
const LANGUAGE_DISPLAY_NAMES = {
  chi: '中文', zho: '中文', zh: '中文', 'zh-cn': '简体中文', 'zh-hans': '简体中文', chs: '简体中文',
  'zh-tw': '繁體中文', 'zh-hk': '粤语(繁)', 'zh-hant': '繁體中文', cht: '繁體中文', yue: '粤语',
  eng: '英语', en: '英语', jpn: '日语', ja: '日语', kor: '韩语', ko: '韩语',
  fre: '法语', fra: '法语', fr: '法语', ger: '德语', deu: '德语', de: '德语',
  spa: '西班牙语', es: '西班牙语', rus: '俄语', ru: '俄语', por: '葡萄牙语', pt: '葡萄牙语',
  ita: '意大利语', it: '意大利语', tha: '泰语', th: '泰语', vie: '越南语', vi: '越南语',
  ara: '阿拉伯语', ar: '阿拉伯语', hin: '印地语', hi: '印地语', ind: '印尼语', id: '印尼语'
};
function languageDisplayName(code){
  const key = String(code || '').trim().toLowerCase();
  if(!key) return '';
  if(LANGUAGE_DISPLAY_NAMES[key]) return LANGUAGE_DISPLAY_NAMES[key];
  const base = key.split(/[-_]/)[0];
  return LANGUAGE_DISPLAY_NAMES[base] || code;
}

/* 从播放器快照读取 mpv 实时轨道表;原生播放未就绪时返回 null */
async function liveMpvTracks(){
  if(!TtvBackend.available() || !isNativeMediaMode()) return null;
  try{
    const state = await TtvBackend.invoke('player_state');
    if(!state || (!Array.isArray(state.audioTracks) && !Array.isArray(state.subtitleTracks))) return null;
    return {
      audioTracks: Array.isArray(state.audioTracks) ? state.audioTracks : [],
      subtitleTracks: Array.isArray(state.subtitleTracks) ? state.subtitleTracks : [],
      currentAudio: Number.isFinite(Number(state.audioTrack)) ? Number(state.audioTrack) : null,
      currentSubtitle: Number.isFinite(Number(state.subtitleTrack)) ? Number(state.subtitleTrack) : null
    };
  }catch(error){
    console.warn('Unable to read mpv track list:', error);
    return null;
  }
}

/* 字幕面板：在触发按钮上方展开，搜索结果留在同一面板内。 */
async function openSubtitleModal(source){
  const anchor = resolvePlayerPopoverAnchor(source, 'btnSubtitles');
  const token = openPlayerActionPopover({
    kind: 'subtitles',
    title: '字幕',
    hint: '选择内嵌字幕，或添加外挂字幕',
    anchor,
    variant: 'wide',
    bodyHtml: '<div class="player-popover-loading"><i></i><span>正在读取字幕轨道…</span></div>'
  });
  if(token === null) return;
  const live = await liveMpvTracks();
  const liveSubs = live?.subtitleTracks || [];
  const probed = selectedMovie?.mediaProbe?.subtitles || [];
  const subs = liveSubs.length ? liveSubs : probed.length ? probed : mediaStreams().filter(track => /subtitle|subtitles|text/i.test(String(track?.type || track?.codecType || '')));
  const body = subs.length ? subs.map((track, index) => {
    const langName = languageDisplayName(track.language || track.lang);
    const label = langName || track.language || track.lang || track.title || `字幕轨道 ${index + 1}`;
    const codec = track.codec || track.codecName || '字幕流';
    const trackId = track.id ?? track.index ?? index;
    const trackTitle = String(track.title || '').trim();
    const titleSuffix = trackTitle && trackTitle.toLocaleLowerCase() !== String(label).trim().toLocaleLowerCase() ? ` · ${trackTitle}` : '';
    const isCurrent = liveSubs.length
      ? live.currentSubtitle === Number(trackId)
      : index === 0;
    return `<button class="player-popover-item${isCurrent ? ' active' : ''}" data-subtitle-label="${escapeHtml(String(label))}" data-track-id="${escapeHtml(String(trackId))}"><span><b>${escapeHtml(String(label) + titleSuffix)}</b><small>${escapeHtml(String(codec))}</small></span><em>${isCurrent ? '当前' : '可用'}</em></button>`;
  }).join('') : '<div class="player-popover-empty">当前媒体没有内嵌字幕，可添加本地字幕或搜索在线字幕。</div>';
  const searchButton = TtvBackend.available() && (selectedMovie?.playUrl || selectedMovie?.providerId)
    ? '<button class="player-popover-action" id="subtitleSearchOnlineBtn">搜索在线字幕</button>'
    : '';
  const providerSearchButton = TtvBackend.available() && selectedMovie?.providerId
    ? '<button class="player-popover-action" id="subtitleProviderSearchBtn">搜索云盘字幕</button>'
    : '';
  if(!updatePlayerActionPopover(token, `<div class="player-popover-stack">
      <div class="player-popover-list">
        ${subs.length ? '<button class="player-popover-item" id="subtitleDisableBtn"><span><b>关闭字幕</b><small>隐藏当前字幕轨道</small></span><em>关闭</em></button>' : ''}
        ${body}
      </div>
      <div class="player-popover-actions">
      <button class="player-popover-action" id="subtitleAddLocalBtn">添加本地字幕文件</button>
      ${providerSearchButton}
      ${searchButton}
      </div>
      <div id="subtitleProviderResults"></div>
      <div id="subtitleOnlineResults"></div>
    </div>`)) return;
  const popoverBody = document.getElementById('playerActionPopoverBody');
  popoverBody?.querySelectorAll('[data-subtitle-label]').forEach(button => button.addEventListener('click', () => {
    const label = button.dataset.subtitleLabel || '自动';
    setSubtitleModeLabel(label);
    const trackId = Number(button.dataset.trackId);
    sendPlayerCommand({type:'setSubtitleTrack', trackId: Number.isFinite(trackId) ? trackId : null});
    toast('已切换字幕：' + label);
    closePlayerActionPopover();
  }));
  document.getElementById('subtitleDisableBtn')?.addEventListener('click', () => {
    setSubtitleModeLabel('字幕关闭');
    sendPlayerCommand({type:'setSubtitleTrack', trackId:null});
    toast('字幕已关闭');
    closePlayerActionPopover();
  });
  document.getElementById('subtitleAddLocalBtn')?.addEventListener('click', () => importLocalSubtitleFile());
  // 云盘字幕:光鸭等 provider 提供官方字幕库匹配 + 云盘同目录字幕文件,一次搜索返回两类
  document.getElementById('subtitleProviderSearchBtn')?.addEventListener('click', async () => {
    const target = document.getElementById('subtitleProviderResults');
    if(target) target.textContent = '正在搜索云盘字幕...';
    try{
      const providerMediaId = selectedMovie.providerMediaId
        || String(selectedMovie.id || '').replace(/^provider:[^:]+:/, '');
      const duration = Number(selectedMovie.mediaProbe?.durationSeconds) > 0
        ? Number(selectedMovie.mediaProbe.durationSeconds)
        : Number(selectedMovie.durationSeconds) || undefined;
      const fileName = basename(selectedMovie.sourceTitle || selectedMovie.t || '');
      const results = await TtvBackend.invoke('provider_subtitle_search', {
        providerId: selectedMovie.providerId,
        input: {mediaId: providerMediaId, name: fileName || undefined, durationSeconds: duration}
      });
      const list = Array.isArray(results) ? results : [];
      if(!list.length){
        if(target) target.innerHTML = '<div class="catalog-empty">云盘没有匹配到可用字幕。</div>';
        return;
      }
      const sourceLabel = {online: '在线字幕库', cloud: '云盘同目录'};
      if(target) target.innerHTML = `<div class="player-popover-results-label">云盘字幕</div><div class="player-popover-list">${list.map((item, index) => `<button class="player-popover-item" data-provider-subtitle-index="${index}"><span><b>${escapeHtml(item.name || '字幕')}</b><small>${sourceLabel[item.source] || item.source} · ${escapeHtml(String(item.ext || '').toUpperCase())}</small></span><em>加载</em></button>`).join('')}</div>`;
      refreshPlayerActionPopoverLayout();
      target?.querySelectorAll('[data-provider-subtitle-index]').forEach(button => button.addEventListener('click', async () => {
        const item = list[Number(button.dataset.providerSubtitleIndex)];
        try{
          const downloaded = await TtvBackend.invoke('provider_subtitle_download', {providerId: selectedMovie.providerId, input: {subtitle: item}});
          if(downloaded?.path){
            await TtvBackend.invoke('subtitle_attach', {input: {path: downloaded.path, select: true}});
            setSubtitleModeLabel(downloaded.name || '云盘字幕');
            toast('字幕已下载并挂载');
            closePlayerActionPopover();
          }
        }catch(error){ toast('云盘字幕下载失败：' + backendErrorMessage(error)); }
      }));
    }catch(error){
      if(target) target.textContent = '云盘字幕搜索失败：' + backendErrorMessage(error);
    }
  });
  document.getElementById('subtitleSearchOnlineBtn')?.addEventListener('click', async () => {
    const target = document.getElementById('subtitleOnlineResults');
    if(target) target.textContent = '正在搜索在线字幕...';
    try{
      const language = (navigator.language || 'zh').split('-')[0] || 'zh';
      if(!selectedMovie.playUrl){
        if(target) target.innerHTML = '<div class="catalog-empty">当前视频还没有可搜索的播放地址。请先开始播放，或改用云盘字幕搜索。</div>';
        return;
      }
      const results = await TtvBackend.invoke('subtitle_search', {input:{url:selectedMovie.playUrl, headers:selectedMovie.playHeaders || {}, language, query:selectedMovie.sourceTitle || selectedMovie.t, year:Number(selectedMovie.y) || undefined}});
      const list = Array.isArray(results) ? results : [];
      const online = list.filter(item => item.source === 'opensubtitles');
      const local = list.filter(item => item.source === 'local');
      const rows = [
        ...local.map(item => `<button class="player-popover-item" data-local-subtitle="${escapeHtml(String(item.path || ''))}"><span><b>${escapeHtml(basename(item.path || '本地字幕'))}</b><small>本地匹配字幕</small></span><em>加载</em></button>`),
        ...online.slice(0, 8).map(item => `<button class="player-popover-item" data-online-subtitle="${escapeHtml(String(item.fileId || ''))}"><span><b>${escapeHtml(item.release || item.language || '在线字幕')}</b><small>OpenSubtitles</small></span><em>下载</em></button>`)
      ];
      if(target) target.innerHTML = rows.length ? `<div class="player-popover-results-label">在线字幕</div><div class="player-popover-list">${rows.join('')}</div>` : '<div class="player-popover-empty">没有找到在线字幕。可先添加本地字幕，或在设置中配置 OpenSubtitles API Key。</div>';
      refreshPlayerActionPopoverLayout();
      target?.querySelectorAll('[data-local-subtitle]').forEach(button => button.addEventListener('click', async () => {
        try{
          await TtvBackend.invoke('subtitle_attach', {input:{path:button.dataset.localSubtitle, select:true}});
          setSubtitleModeLabel('本地字幕');
          toast('本地字幕已挂载到当前视频');
          closePlayerActionPopover();
        }catch(error){ toast('本地字幕挂载失败：' + backendErrorMessage(error)); }
      }));
      target?.querySelectorAll('[data-online-subtitle]').forEach(button => button.addEventListener('click', async () => {
        try{
          const downloaded = await TtvBackend.invoke('subtitle_download', {input:{fileId:button.dataset.onlineSubtitle, mediaId:String(selectedMovie.id)}});
          if(downloaded?.path){ await TtvBackend.invoke('subtitle_attach', {input:{path:downloaded.path, select:true}}); setSubtitleModeLabel(downloaded.release || '在线字幕'); toast('在线字幕已下载并挂载'); closePlayerActionPopover(); }
        }catch(error){ toast('在线字幕下载失败：' + backendErrorMessage(error)); }
      }));
    }catch(error){ if(target) target.textContent = '在线字幕搜索失败：' + backendErrorMessage(error); }
  });
}

async function importLocalSubtitleFile(){
  const picker = document.createElement('input');
  picker.type = 'file';
  picker.accept = '.srt,.ass,.ssa,.vtt,.sub,text/vtt,application/x-subrip';
  picker.addEventListener('change', async () => {
    const file = picker.files && picker.files[0];
    if(!file) return;
    try{
      const content = await file.text();
      if(TtvBackend.available()){
        await TtvBackend.invoke('subtitle_import', {input:{fileName:file.name, content, select:true}});
      }else if(!attachHtmlSubtitle(content, file.name)){
        throw new Error('当前播放器无法挂载字幕');
      }
      setSubtitleModeLabel(file.name);
      toast('字幕已添加到当前视频：' + file.name);
      closePlayerActionPopover();
    }catch(error){
      toast('添加字幕失败：' + backendErrorMessage(error));
    }
  });
  picker.click();
}

function mediaStreams(movie = selectedMovie){
  const metadata = movie?.metadata || {};
  return Array.isArray(metadata.streams) ? metadata.streams : (Array.isArray(metadata.tracks) ? metadata.tracks : []);
}

/* 音轨选择面板(视频语言)。
   原生播放时直接读 mpv track-list 的实时音轨(id/lang/title/codec/选中态),
   轨道 id 就是 mpv `aid` 接受的值——此前用 ffprobe 的容器 stream index 当 aid,
   在"视频流在音频流之前"的文件上会切错轨道,现在原生路径不再经过 probe 索引。 */
async function openAudioTrackModal(source){
  const anchor = resolvePlayerPopoverAnchor(source, 'btnAudioTracks');
  const token = openPlayerActionPopover({
    kind: 'audio-tracks',
    title: '音轨',
    hint: '切换视频语言与声道流',
    anchor,
    bodyHtml: '<div class="player-popover-loading"><i></i><span>正在读取音轨…</span></div>'
  });
  if(token === null) return;
  const live = await liveMpvTracks();
  const liveAudio = live?.audioTracks || [];
  const probed = selectedMovie?.mediaProbe?.audio || [];
  const tracks = liveAudio.length ? liveAudio : probed.length ? probed : mediaStreams().filter(track => /audio/i.test(String(track?.type || track?.codecType || '')));
  const body = tracks.length ? tracks.map((track, index) => {
    const langName = languageDisplayName(track.language || track.lang);
    const label = langName || track.language || track.lang || track.title || `音轨 ${index + 1}`;
    const codec = track.codec || track.codecName || '音频流';
    const channels = track.channels ? ` · ${track.channels} 声道` : '';
    const titleSuffix = track.title ? ` · ${track.title}` : '';
    const trackId = track.id ?? track.index ?? index;
    const isCurrent = liveAudio.length
      ? live.currentAudio === Number(trackId)
      : index === 0;
    return `<button class="player-popover-item${isCurrent ? ' active' : ''}" data-track-label="${escapeHtml(String(label))}" data-track-id="${escapeHtml(String(trackId))}"><span><b>${escapeHtml(String(label))}</b><small>${escapeHtml(String(codec) + channels + titleSuffix)}</small></span><em>${isCurrent ? '当前' : '可用'}</em></button>`;
  }).join('') : '<div class="player-popover-empty">当前媒体没有提供可读取的音轨元数据，播放器将自动选择可用音轨。</div>';
  if(!updatePlayerActionPopover(token, `<div class="player-popover-list">${body}</div>`)) return;
  document.getElementById('playerActionPopoverBody')?.querySelectorAll('[data-track-label]').forEach(button => button.addEventListener('click', () => {
    const label = button.dataset.trackLabel || '自动';
    const audioMode = document.getElementById('pAudioModeText');
    if(audioMode) audioMode.textContent = label;
    const trackId = Number(button.dataset.trackId);
    sendPlayerCommand({type:'setAudioTrack', trackId: Number.isFinite(trackId) ? trackId : null});
    toast('已切换音轨：' + label);
    closePlayerActionPopover();
  }));
  return;
}

/* 声卡设备信息沿用同一锚定面板。 */
function openAudioDeviceModal(source){
  const anchor = resolvePlayerPopoverAnchor(source, 'btnAudioTracks');
  if(!TtvBackend.available()){
    openPlayerActionPopover({kind:'audio-device', title:'音频输出', hint:'当前播放设备', anchor, bodyHtml:'<div class="player-popover-empty">浏览器页面无法枚举系统声卡；桌面播放内核会使用系统默认输出设备。</div>'});
    return;
  }
  const token = openPlayerActionPopover({kind:'audio-device', title:'音频输出', hint:'当前播放设备与直通能力', anchor, bodyHtml:'<div class="player-popover-loading"><i></i><span>正在读取桌面音频能力…</span></div>'});
  if(token === null) return;
  TtvBackend.invoke('player_audio_capabilities').then(capabilities => {
    const codecs = Array.isArray(capabilities?.codecs) && capabilities.codecs.length ? capabilities.codecs.join('、') : '无';
    updatePlayerActionPopover(token, `<div class="player-popover-facts"><div><span>设备</span><b>${escapeHtml(String(capabilities?.device || '系统默认输出设备'))}</b></div><div><span>数字直通</span><b>${capabilities?.passthrough ? '已启用' : '未启用'}</b></div><div><span>支持编码</span><b>${escapeHtml(codecs)}</b></div>${capabilities?.reason ? `<small>${escapeHtml(String(capabilities.reason))}</small>` : ''}</div>`);
  }).catch(error => {
    updatePlayerActionPopover(token, '<div class="player-popover-empty error">无法读取音频能力：' + escapeHtml(backendErrorMessage(error)) + '</div>');
  });
}

/* 声道映射信息面板 */
function openChannelMapModal(source){
  const channels = selectedMovie?.metadata?.channels || selectedMovie?.metadata?.channelLayout || '跟随媒体源';
  openPlayerActionPopover({
    kind:'channel-map',
    title:'声道布局',
    hint:'跟随当前媒体源',
    anchor:resolvePlayerPopoverAnchor(source, 'btnAudioTracks'),
    bodyHtml:`<div class="player-popover-facts"><div><span>当前布局</span><b>${escapeHtml(String(channels))}</b></div><small>当前媒体没有提供可切换的矩阵映射，播放器将保持源布局。</small></div>`
  });
}

const AUDIO_PRESET_LABELS = {off:'原声', movie:'电影', music:'音乐', night:'夜听', voice:'人声', surround:'环绕'};
/* 音频预设点击 */
function pickAudioPreset(el, name){
  pickChip(el);
  const key = String(name || 'off');
  const label = AUDIO_PRESET_LABELS[key] || key;
  const presetText = document.getElementById('pAudioPresetText');
  if(presetText) presetText.textContent = label;
  sendPlayerCommand({type:'setAudioPreset', preset: key});
  toast('已应用声场预设：' + label);
}

/* 云盘来源(光鸭 videoResource 映射)的 qualities 优先,否则回退 versions 元数据 */
function qualityEntriesFor(movie){
  if(Array.isArray(movie?.qualities) && movie.qualities.length){
    const entries = new Map();
    movie.qualities.forEach(q => {
      const gcid = String(q.gcid || '');
      const label = String(q.displayName || q.resolutionName || q.shortName || '自动').trim() || '自动';
      const entry = {
        gcid,
        label,
        needVip: Number(q.needVipType) === 2,
        isDefault: q.isDefault === true,
        durationSeconds: Number(q.durationSeconds) || 0
      };
      const key = label.toLowerCase();
      const current = entries.get(key);
      // 同名档位保留默认流；没有默认流时优先保留无需会员的可用流。
      if(!current || (entry.isDefault && !current.isDefault) || (!entry.needVip && current.needVip)){
        entries.set(key, entry);
      }
    });
    return [...entries.values()];
  }
  const versions = Array.isArray(movie?.versions) ? movie.versions : [];
  const seen = new Set();
  return versions.map(v => ({gcid: '', label: String(v.quality || v.resolution || v.name || '原始资源'), version: v}))
    .filter(entry => {
      const key = entry.label.trim().toLowerCase();
      if(!key || seen.has(key)) return false;
      seen.add(key);
      return true;
    });
}

/* 真实清晰度切换:与官方播放器同机制——保存进度后携带目标 gcid 重新解析直链,
   openPlayer 会用同一历史进度续播,实现"换流不换位置"。 */
async function switchToQuality(entry){
  const chip = document.getElementById('chipQuality');
  if(chip){
    chip.textContent = entry.label;
    chip.dataset.userSelected = 'true';
  }
  if(entry.gcid && selectedMovie?.providerId){
    await saveWatchProgress();
    void openPlayer({...selectedMovie, playbackQualityGcid: entry.gcid, playbackQuality: entry.label}, null, true);
    return;
  }
  if(entry.version?.__media){
    await saveWatchProgress();
    void openPlayer({...selectedMovie, ...(entry.version.__media || {}), playbackQuality: String(entry.label)}, null, true);
    return;
  }
  toast('当前视频没有可切换的真实画质流，标签不会改变实际分辨率。');
}

/* 分辨率与画质切换弹窗 */
function openQualityModal(){
  const entries = qualityEntriesFor(selectedMovie);
  const body = entries.length ? entries.map((entry, index) => `<button class="btn" data-quality-index="${index}" data-quality-label="${escapeHtml(entry.label)}" style="display:flex;flex-direction:column;align-items:flex-start;padding:12px 16px"><b style="color:#fff">${escapeHtml(entry.label)}${entry.needVip ? ' <small style="color:#f5c26b">会员</small>' : ''}${entry.isDefault ? ' <small style="color:var(--text-faint)">默认</small>' : ''}</b><small style="color:var(--text-faint)">${entry.gcid ? '切换后从当前进度继续播放' : escapeHtml(versionDetails(selectedMovie, entry.version))}</small></button>`).join('') : `<div class="catalog-empty">当前媒体没有提供可切换的真实版本；播放器将使用自动质量。</div>`;
  openModal('分辨率与视频流', `<div style="display:flex;flex-direction:column;gap:10px">${body}</div>`, '<button class="btn" style="width:100%" onclick="closeModal()">关闭</button>');
  document.querySelectorAll('[data-quality-index]').forEach(button => button.addEventListener('click', async () => {
    const entry = entries[Number(button.dataset.qualityIndex)];
    const label = button.dataset.qualityLabel;
    closeModal();
    // 画质切换的真实机制：先把当前进度写入历史，再按新画质重新解析播放地址并重开播放器，
    // 而不是给 mpv 发一个无意义的 video-zoom 属性（那样不会真正切换视频流）。
    await switchToQuality({...entry, label});
  }));
}

/* HDR 参数详情弹窗 */
function openHdrInfoModal(){
  const metadata = selectedMovie?.metadata || {};
  const rows = [
    ['HDR 格式', metadata.hdr || metadata.hdrFormat || '未提供'],
    ['色彩空间', metadata.colorSpace || metadata.colorPrimaries || '未提供'],
    ['传输特性 (EOTF)', metadata.transferCharacteristics || '未提供'],
    ['峰值亮度 (MaxCLL)', metadata.maxCLL || '未提供'],
    ['色深', metadata.bitDepth || metadata.bitsPerRawSample || '未提供']
  ];
  openModal('HDR 与高动态范围元数据', `<div style="display:grid;gap:12px;font-size:13px;color:var(--text-dim)">${rows.map(([label, value]) => `<div style="display:flex;justify-content:space-between;padding:10px 14px;background:rgba(255,255,255,0.05);border-radius:10px"><span>${label}</span><b style="color:#fff">${escapeHtml(String(value))}</b></div>`).join('')}</div>`, '<button class="btn btn-accent" style="width:100%" onclick="closeModal()">关闭</button>');
}

/* 高级画质与视觉增强面板：真实控件面板。
   GLSL 画质着色器此前没有任何开关入口，这里补上；补帧/超分原本只有播放器 chip，
   此面板集中管理三者，并展示当前增强运行状态。全部接到 enhancement_set / enhancement_status。
   补帧走播放器内置 display-resample，避免 RIFE 滤镜堵住出帧。 */
async function openVideoEnhanceModal(source){
  const anchor = resolvePlayerPopoverAnchor(source, 'btnVideoEnhance');
  const token = openPlayerActionPopover({
    kind:'video-enhance',
    title:'画质增强',
    hint:'调整后立即应用到当前播放',
    anchor,
    variant:'wide',
    bodyHtml:'<div class="player-popover-loading"><i></i><span>正在读取增强状态…</span></div>'
  });
  if(token === null) return;
  if(!TtvBackend.available()){
    updatePlayerActionPopover(token, '<div class="player-popover-empty">当前页面未连接桌面端增强管线，暂时不能调整视频增强。</div>');
    return;
  }
  let status = {};
  try{ status = await TtvBackend.invoke('enhancement_status'); }catch(error){ console.warn('Unable to read enhancement status:', error); }
  const row = (id, title, desc, on) => `
    <div class="player-popover-setting">
      <div><b>${title}</b><small>${desc}</small></div>
      <button class="player-popover-switch${on ? ' on' : ''}" id="${id}" type="button" role="switch" aria-checked="${on ? 'true' : 'false'}"><span class="chip-label">${on ? '已开启' : '已关闭'}</span></button>
    </div>`;
  const body = `
    <div class="player-popover-stack">
      ${row('enhGlsl', '画质增强着色器', 'GLSL 着色器提升清晰度与色彩还原', !!status.glslEnabled)}
      ${row('enhRife', '实时补帧', '按显示器刷新率插帧（24 帧电影会明显变顺），硬解下也能生效', !!status.rifeEnabled)}
      ${row('enhUai', '实时超分辨率', 'GPU 渲染阶段增强边缘与细节，播放时即时生效', !!status.uaiEnabled)}
      <div class="player-popover-status" id="enhStatus"></div>
    </div>`;
  if(!updatePlayerActionPopover(token, body)) return;
  renderEnhanceStatusLine(status);
  const bind = (id, name, label) => {
    const el = document.getElementById(id);
    if(!el) return;
    el.addEventListener('click', async () => {
      const enabled = el.classList.toggle('on');
      el.setAttribute('aria-checked', enabled ? 'true' : 'false');
      const chipLabel = el.querySelector('.chip-label');
      if(chipLabel) chipLabel.textContent = enabled ? '已开启' : '已关闭';
      try{
        await TtvBackend.invoke('enhancement_set', {name, enabled});
        toast(label + (enabled ? '已开启' : '已关闭'));
        const restarted = await afterEnhancementToggle(name, enabled);
        if(restarted) return;
      }catch(error){
        el.classList.toggle('on', !enabled);
        el.setAttribute('aria-checked', enabled ? 'false' : 'true');
        if(chipLabel) chipLabel.textContent = enabled ? '已关闭' : '已开启';
        toast('增强设置未保存：' + backendErrorMessage(error));
      }
      try{ renderEnhanceStatusLine(await TtvBackend.invoke('enhancement_status')); }catch(error){/* 忽略状态刷新失败 */}
      // 同步播放器上的补帧/超分 chip 与帧率面板，避免两处状态不一致
      restoreEnhancementUi();
    });
  };
  bind('enhGlsl', 'glsl', '画质增强着色器');
  bind('enhRife', 'rife', '内置实时补帧');
  bind('enhUai', 'uai', '实时超分辨率');
}

/* 增强弹窗底部的运行状态行 */
function renderEnhanceStatusLine(status){
  const el = document.getElementById('enhStatus');
  if(!el) return;
  const parts = [];
  const features = [];
  if(status.glslEnabled) features.push('画质着色器');
  if(status.rifeEnabled) features.push('补帧');
  if(status.uaiEnabled) features.push('超分');
  parts.push(features.length ? '已启用：' + features.join('、') : '当前未启用任何增强');
  if(Number.isFinite(status.actualFps) && status.actualFps > 0) parts.push('解码 ' + Number(status.actualFps).toFixed(1) + ' FPS');
  if(Number.isFinite(status.displayFps) && status.displayFps > 0) parts.push('显示 ' + Number(status.displayFps).toFixed(1) + ' Hz');
  if(status.fallbackActive) parts.push('⚠ 已回退：' + (status.reason || '运行时资源不可用'));
  el.textContent = parts.join(' · ');
}

/* 弹幕浮层 */
function toggleDanmaku(btn){
  const source = selectedMovie?.metadata?.danmaku || selectedMovie?.metadata?.danmakuItems;
  danmakuItems = Array.isArray(source) ? source.map(item => String(item?.text || item)).filter(Boolean) : [];
  if(!danmakuItems.length){
    isDanmakuOn = false;
    btn.classList.remove('on');
    toast('当前未配置弹幕源');
    return;
  }
  isDanmakuOn = !isDanmakuOn;
  btn.classList.toggle('on', isDanmakuOn);
  if(isDanmakuOn){
    startDanmaku();
    toast('实时弹幕已开启');
  } else {
    stopDanmaku();
    toast('实时弹幕已关闭');
  }
}

function startDanmaku(){
  stopDanmaku();
  spawnDanmaku();
  danmakuTimer = setInterval(spawnDanmaku, 1600);
}
function stopDanmaku(){
  if(danmakuTimer) clearInterval(danmakuTimer);
  const stage = document.getElementById('danmakuStage');
  if(stage) stage.innerHTML = '';
}
function spawnDanmaku(){
  const stage = document.getElementById('danmakuStage');
  if(!stage || !isDanmakuOn) return;
  const d = document.createElement('div');
  d.className = 'danmaku-item';
  d.textContent = danmakuItems[Math.floor(Math.random() * danmakuItems.length)];
  d.style.top = (Math.random() * 60 + 12) + '%';
  d.style.color = Math.random() > 0.6 ? '#38bdf8' : '#ffffff';
  stage.appendChild(d);
  setTimeout(() => d.remove(), 9500);
}

let fpsSampleAt = 0;
let fpsSampleFrames = 0;
let fpsSampleDropped = 0;
function formatFpsValue(value, unit){
  const n = Number(value);
  if(!Number.isFinite(n) || n <= 0) return unit ? '— ' + unit : '—';
  return (Math.round(n * 10) / 10) + (unit ? ' ' + unit : '');
}
function writeFpsPanel({sourceFps, actualFps, decodeFps, displayFps, droppedFrames}){
  const mediaEl = document.getElementById('fpsMedia');
  const actualEl = document.getElementById('fpsActual');
  const decodeEl = document.getElementById('fpsDecode');
  const displayEl = document.getElementById('fpsDisplay');
  const dropsEl = document.getElementById('fpsDrops');
  const probeFps = Number(selectedMovie?.mediaProbe?.video?.[0]?.frameRate);
  const source = Number.isFinite(sourceFps) && sourceFps > 0 ? sourceFps : probeFps;
  if(mediaEl) mediaEl.textContent = formatFpsValue(source, 'FPS');
  if(actualEl){
    actualEl.textContent = formatFpsValue(actualFps, 'FPS');
    actualEl.className = Number.isFinite(Number(actualFps)) && Number(actualFps) > 0 ? 'green' : '';
  }
  if(decodeEl) decodeEl.textContent = formatFpsValue(decodeFps ?? source, 'FPS');
  if(displayEl && displayFps !== undefined) displayEl.textContent = formatFpsValue(displayFps, 'Hz');
  if(dropsEl && droppedFrames !== undefined){
    dropsEl.textContent = Number.isFinite(Number(droppedFrames)) && Number(droppedFrames) >= 0
      ? String(droppedFrames)
      : '—';
  }
}
setInterval(() => {
  if(!player.classList.contains('active')) return;
  if(!playerVideo || !player.classList.contains('has-real-video')){
    // 原生 libmpv 模式下由 syncBackendPlayback → updateEnhancementFeedback 写入。
    if(!isNativeMediaMode()){
      writeFpsPanel({actualFps: null, displayFps: null, droppedFrames: null});
    }
    return;
  }
  const quality = playerVideo.getVideoPlaybackQuality?.();
  if(!quality){
    writeFpsPanel({actualFps: null});
    return;
  }
  const now = performance.now();
  let measured = null;
  if(fpsSampleAt){
    const elapsed = (now - fpsSampleAt) / 1000;
    const frames = Math.max(0, quality.totalVideoFrames - fpsSampleFrames);
    if(elapsed > 0) measured = frames / elapsed;
  }
  fpsSampleAt = now;
  fpsSampleFrames = quality.totalVideoFrames;
  fpsSampleDropped = quality.droppedVideoFrames;
  writeFpsPanel({actualFps: measured, decodeFps: measured, droppedFrames: fpsSampleDropped});
}, 1400);

function toggleAudioPanel(source){
  const panel = document.getElementById('audioPanel');
  if(!panel) return;
  const wasHidden = panel.classList.contains('tvv-audio-hidden');
  const anchor = resolvePlayerPopoverAnchor(source, 'btnAudioEffects');
  closeAllIslandPopovers?.('audioPanel');
  panel.classList.toggle('tvv-audio-hidden', !wasHidden);
  panel.setAttribute('aria-hidden', wasHidden ? 'false' : 'true');
  if(wasHidden){
    audioPanelAnchor = anchor;
    anchor?.setAttribute('aria-expanded', 'true');
    requestAnimationFrame(() => requestAnimationFrame(positionAudioPanel));
  }else{
    anchor?.setAttribute('aria-expanded', 'false');
    audioPanelAnchor = null;
  }
  uiActivity();
  toast(panel.classList.contains('tvv-audio-hidden') ? '音效面板已收起' : '音效面板已打开');
}

/* 打开播放器时按后端已保存的增强开关恢复按钮点亮状态，
   避免每次开视频按钮都回到"全关"的假象（设置本身是持久化的）。 */
let enhancementRifeEnabled = false;
function isHtml5PlaybackActive(){
  return Boolean(player?.classList.contains('has-real-video') && !player.classList.contains('native-video-live'));
}
async function afterEnhancementToggle(name, enabled){
  if(name !== 'rife'){
    restoreEnhancementUi();
    return false;
  }
  enhancementRifeEnabled = !!enabled;
  if(enabled && isHtml5PlaybackActive() && selectedMovie){
    selectedMovie.forceNativePlayback = true;
    toast('正在切换到原生播放器并启动补帧…');
    await openPlayer(selectedMovie, null, true);
    return true;
  }
  if(!enabled && selectedMovie) selectedMovie.forceNativePlayback = false;
  restoreEnhancementUi();
  return false;
}
async function restoreEnhancementUi(){
  if(!TtvBackend.available()) return;
  lastDegradationReason = null;
  const fpsDecode = document.getElementById('fpsDecode');
  const fpsDisplay = document.getElementById('fpsDisplay');
  if(fpsDecode) fpsDecode.textContent = '— FPS';
  if(fpsDisplay) fpsDisplay.textContent = '— Hz';
  const dropsEl = document.getElementById('fpsDrops');
  if(dropsEl) dropsEl.textContent = '—';
  try{
    const status = await TtvBackend.invoke('enhancement_status');
    enhancementRifeEnabled = !!status.rifeEnabled;
    const rifeChip = document.getElementById('chipRife');
    if(rifeChip){
      rifeChip.classList.toggle('on', !!status.rifeEnabled);
      const label = rifeChip.querySelector('.chip-label');
      if(label) label.textContent = status.rifeEnabled ? '关闭补帧' : '补帧';
    }

    const uaiChip = document.getElementById('chipUai');
    if(uaiChip) uaiChip.classList.toggle('on', !!status.uaiEnabled);
    const fpsIns = document.getElementById('fpsIns');
    if(fpsIns){
      if(status.fallbackActive){
        fpsIns.textContent = '已回退';
        fpsIns.className = 'gold';
      }else{
        fpsIns.textContent = status.rifeEnabled ? '已配置' : '未配置';
        fpsIns.className = status.rifeEnabled ? 'green' : '';
      }
    }
  }catch(error){
    console.warn('Unable to restore enhancement UI:', error);
  }
}

/* 原生播放期间每秒随 player_state 刷新增强运行反馈：
   补帧真实状态（运行中/已回退/未运行）、解码帧率、显示刷新与降级原因提示。 */
let lastDegradationReason = null;
function updateEnhancementFeedback(state){
  const fpsIns = document.getElementById('fpsIns');
  if(fpsIns){
    if(state.interpolationStatus === 'display-resample' || state.interpolationStatus === 'rife' || state.interpolationStatus === 'minterpolate' || state.interpolationStatus === 'lavfi'){
      fpsIns.textContent = '运行中';
      fpsIns.className = 'green';
    }else if(state.interpolationStatus === 'fallback'){
      fpsIns.textContent = '已回退';
      fpsIns.className = 'gold';
    }else if(state.interpolationStatus === 'disabled'){
      fpsIns.textContent = '未运行';
      fpsIns.className = 'gold';
    }else if(state.interpolationEnabled){
      fpsIns.textContent = '已配置';
      fpsIns.className = 'green';
    }else{
      fpsIns.textContent = '未配置';
      fpsIns.className = '';
    }
  }
  writeFpsPanel({
    sourceFps: state.sourceFps,
    actualFps: state.actualFps,
    decodeFps: state.sourceFps,
    displayFps: state.displayFps,
    droppedFrames: state.droppedFrames
  });
  const reason = state.degradationReason || null;
  if(reason && reason !== lastDegradationReason){
    toast('视频增强已降级：' + reason);
  }
  lastDegradationReason = reason;
}

async function toggleRife(button){
  if(!TtvBackend.available()){
    toast('补帧设置仅可在桌面端运行时中保存。');
    return;
  }
  const enabled = button.classList.toggle('on');
  // 只更新文本标签，保留闪电图标与 AI 角标
  const label = button.querySelector('.chip-label');
  if(label) label.textContent = enabled ? '关闭补帧' : '补帧';
  const fpsIns = document.getElementById('fpsIns');
  if(fpsIns){
    fpsIns.textContent = enabled ? '启动中' : '未配置';
    fpsIns.className = enabled ? 'green' : '';
  }
  try{
    await TtvBackend.invoke('enhancement_set', {name: 'rife', enabled});
    const restarted = await afterEnhancementToggle('rife', enabled);
    if(restarted) return;
    toast(enabled ? '补帧已开启：按屏幕刷新率插帧，24 帧片源会更顺' : '补帧已关闭');
  }catch(error){
    button.classList.toggle('on', !enabled);
    if(label) label.textContent = enabled ? '补帧' : '关闭补帧';
    enhancementRifeEnabled = !enabled;
    toast('补帧设置未保存：' + backendErrorMessage(error));
  }
}

async function toggleAiUpscale(button){
  if(!TtvBackend.available()){
    toast('实时超分设置仅可在桌面端运行时中保存。');
    return;
  }
  const enabled = button.classList.toggle('on');
  try{
    await TtvBackend.invoke('enhancement_set', {name: 'uai', enabled});
    toast(enabled ? '实时超分已开启' : '实时超分已关闭');
  }catch(error){
    button.classList.toggle('on', !enabled);
    toast('实时超分设置未保存：' + backendErrorMessage(error));
  }
}

/* ================= 小交互 ================= */
function pickPill(el){ el.parentElement.querySelectorAll('.tab-pill').forEach(p => p.classList.remove('active')); el.classList.add('active'); }
function pickChip(el){ el.parentElement.querySelectorAll('.ap-chip').forEach(p => p.classList.remove('active')); el.classList.add('active'); }

function toast(msg){
  if(!toastEl) return;
  if(document.body.classList.contains('player-active') || player?.classList.contains('active')){
    toastEl.classList.remove('show');
    return;
  }
  toastEl.textContent = msg;
  toastEl.classList.add('show');
  if(toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toastEl.classList.remove('show'), 2200);
}

/* 顶栏精准悬停展开：仅当鼠标进入用户胶囊区域或展开区时才展开 */
const topbarIsland = document.getElementById('topbarIsland');
const userPill = document.querySelector('.user-pill');
const expandGroup = document.querySelector('.topbar-expand-group');
let topbarExpandTimer = null;

if(topbarIsland && userPill){
  const expand = () => {
    if(topbarExpandTimer) clearTimeout(topbarExpandTimer);
    topbarIsland.classList.add('is-expanded');
  };
  const collapse = () => {
    if(topbarExpandTimer) clearTimeout(topbarExpandTimer);
    topbarExpandTimer = setTimeout(() => {
      topbarIsland.classList.remove('is-expanded');
    }, 360);
  };

  userPill.addEventListener('mouseenter', expand);
  if(expandGroup) expandGroup.addEventListener('mouseenter', expand);
  topbarIsland.addEventListener('mouseleave', collapse);
}

/* Hero 背景由 hero-slides/hero-shade 结构承载；旧版视差元素已随设计移除 */

/* 键盘 */
document.addEventListener('keydown', e => {
  if(!player.classList.contains('active')) return;
  const target = e.target;
  if(target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName))) return;
  const code = e.code;
  const isDigit = /^Digit[0-9]$/.test(code);
  const handled = ['Space','KeyK','KeyJ','KeyL','ArrowLeft','ArrowRight','ArrowUp','ArrowDown','KeyM','KeyF','KeyP','Home','End','Comma','Period','NumpadSubtract','NumpadAdd','Equal','Minus','Escape'].includes(code) || isDigit;
  if(!handled) return;
  uiActivity();
  if(code === 'Escape'){
    e.preventDefault();
    if(closeOpenPlayerPopover()){
      uiActivity();
      return;
    }
    closePlayer();
    return;
  }
  if(code === 'Space' || code === 'KeyK'){ e.preventDefault(); togglePlay(); return; }
  if(code === 'ArrowLeft' || code === 'KeyJ'){ e.preventDefault(); seek(e.shiftKey ? -30 : -10); return; }
  if(code === 'ArrowRight' || code === 'KeyL'){ e.preventDefault(); seek(e.shiftKey ? 30 : 10); return; }
  if(code === 'ArrowUp'){ e.preventDefault(); setPlayerVolume(playerVolume + 5, true); return; }
  if(code === 'ArrowDown'){ e.preventDefault(); setPlayerVolume(playerVolume - 5, true); return; }
  if(code === 'KeyM'){ e.preventDefault(); togglePlayerMute(); return; }
  if(code === 'KeyF'){ e.preventDefault(); toggleFullscreen(); return; }
  if(code === 'KeyP'){ e.preventDefault(); togglePipMode(); return; }
  if(code === 'Home'){
    e.preventDefault();
    if(TOTAL > 0){ cur = 0; sendPlayerCommand({type:'seek', positionSeconds:0}); renderProgress(); }
    return;
  }
  if(code === 'End'){
    e.preventDefault();
    if(TOTAL > 0){ cur = TOTAL; sendPlayerCommand({type:'seek', positionSeconds:TOTAL}); renderProgress(); }
    return;
  }
  if(code === 'Comma' || code === 'NumpadSubtract' || code === 'Minus'){
    e.preventDefault();
    changePlaybackSpeed(-0.25);
    return;
  }
  if(code === 'Period' || code === 'NumpadAdd' || code === 'Equal'){
    e.preventDefault();
    changePlaybackSpeed(0.25);
    return;
  }
  if(isDigit && TOTAL > 0){
    e.preventDefault();
    const percent = Number(code.slice(-1)) / 10;
    cur = TOTAL * percent;
    sendPlayerCommand({type:'seek', positionSeconds:cur});
    renderProgress();
  }
});

/* ============ 我的内容追剧列表渲染 ============ */
function renderWatchlist(){
  const grid = document.getElementById('myWatchlistGrid');
  if(!grid) return;
  grid.innerHTML = '';
  const watchlist = MOVIES.filter(m => isMovieFavorite(m) && !m.adult);
  if(!watchlist.length){
    grid.innerHTML = '<p class="catalog-empty">暂无收藏内容；在媒体库或详情页点击收藏后会显示在这里。</p>';
    return;
  }
  watchlist.forEach((m, i) => {
    const card = document.createElement('div');
    card.className = 'movie-card rise-in';
    card.style.animationDelay = (i * 60) + 'ms';
    card.innerHTML = `
      <div class="fc-scene">
        <div class="fc-inner">
          <div class="fc-face fc-front">
            ${posterMarkup(m)}
            <span class="badge q-badge">${m.q}</span>
          </div>
          <div class="fc-face fc-back">
            <div class="fc-back-bg"><img data-cover-src="${m.img}" alt="" width="400" height="600" loading="lazy" decoding="async"/></div>
            <div class="fc-body">
              <div class="fc-title">${m.t}</div>
              <div class="fc-chips"><span>${m.y}</span><span>${m.d}</span><span>${m.genre}</span></div>
              <p class="fc-desc">${escapeHtml(m.summary)}</p>
              <div class="fc-actions">
                <button class="fc-play" data-act="play"><svg viewBox="0 0 24 24" fill="#fff"><path d="M8 5.5v13l11-6.5z"/></svg>播放</button>
                <button class="fc-fav" data-act="unfav" title="移出追剧"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6L6 18M6 6l12 12"/></svg></button>
              </div>
            </div>
          </div>
        </div>
      </div>
      <div class="m-meta">
        <div class="m-title">${m.t}</div>
        <div class="m-sub"><b>★ ${m.r ? Number(m.r).toFixed(1) : '—'}</b> ${m.y || '—'} · ${m.status || (m.playUrl || m.providerId ? '可播放' : '仅元数据')}</div>
      </div>`;
    card.addEventListener('click', () => openDetail(m, card));
    card.querySelector('[data-act="play"]').addEventListener('click', e => { e.stopPropagation(); openPlayer(m, card); });
    card.querySelector('[data-act="unfav"]').addEventListener('click', e => {
      e.stopPropagation();
      void toggleFavorite(m);
    });
    bindFlipCardCovers(card, m, {eagerFront: i < COVER_EAGER_COUNT});
    grid.appendChild(card);
  });
}

async function renderContinueWatching(){
  const grid = document.getElementById('continueGrid');
  if(!grid) return;
  grid.innerHTML = '<p class="catalog-empty">正在读取真实播放记录...</p>';
  if(!TtvBackend.available() || !MOVIES.length){
    grid.innerHTML = '<p class="catalog-empty">暂无播放记录；播放过的真实媒体会出现在这里。</p>';
    return;
  }
  const candidates = MOVIES.filter(movie => movie?.id && !movie.adult).slice(0, 24);
  const historyRows = await Promise.all(candidates.map(async movie => {
    try{
      const history = await TtvBackend.invoke('history_get', {mediaId: String(movie.id)});
      const duration = positiveNumber(history?.durationSeconds || movie.durationSeconds);
      const position = positiveNumber(history?.positionSeconds);
      if(!history || position <= 0 || (history.completed && duration > 0)) return null;
      return {movie, history, duration, position};
    }catch(error){
      console.warn('Unable to load continue-watching history:', error);
      return null;
    }
  }));
  const rows = historyRows.filter(Boolean).sort((left, right) => Number(right.history?.watchedAt || 0) - Number(left.history?.watchedAt || 0)).slice(0, 6);
  if(!rows.length){
    grid.innerHTML = '<p class="catalog-empty">暂无播放记录；播放过的真实媒体会出现在这里。</p>';
    return;
  }
  grid.innerHTML = '';
  rows.forEach(({movie, duration, position}) => {
    const card = document.createElement('div');
    card.className = 'continue-card glass';
    const progress = duration ? Math.min(100, Math.round(position / duration * 100)) : 0;
    card.innerHTML = `
      <div class="continue-thumb">
        <img class="card-cover is-pending" data-cover-src="${movie.img}" alt="${escapeHtml(movie.t)}" width="640" height="360" loading="lazy" decoding="async">
        <div class="continue-prog-bar"><i style="width:${progress}%"></i></div>
        <div class="continue-play-overlay"><span class="continue-play-btn"><svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 5.5v13l11-6.5z"/></svg></span></div>
      </div>
      <div class="continue-body">
        <h4>${escapeHtml(movie.t)}</h4>
        <div class="continue-meta"><span>${escapeHtml(formatPlaybackClock(position))}${duration ? ' / ' + escapeHtml(formatPlaybackClock(duration)) : ''}</span><span class="badge">${escapeHtml(detailSourceLabel(movie))}</span></div>
      </div>`;
    card.addEventListener('click', () => openDetail(movie, card));
    card.querySelector('.continue-play-overlay')?.addEventListener('click', event => {
      event.stopPropagation();
      openPlayer(movie, card);
    });
    card.querySelectorAll('img').forEach(image => bindCardCover(image, {owner: movie}));
    grid.appendChild(card);
  });
}

function openNewDownloadModal(){
  openModal(
    '新建离线下载任务 · New Download Task',
    `
      <div class="modal-field">
        <label>下载链接 (Magnet / HTTP / WebDAV / ed2k)</label>
        <input class="modal-input" placeholder="输入已授权的 HTTP / WebDAV 下载地址" id="inpDlUrl" />
      </div>
      <div class="modal-field">
        <label>存储目录 (Save Path)</label>
        <input class="modal-input" placeholder="输入本地保存目录" />
      </div>
      <div class="modal-field">
        <label>自动刮削海报与音轨</label>
        <div class="switch on" onclick="this.classList.toggle('on')"></div>
      </div>
    `,
    `
      <button class="btn btn-ghost" onclick="closeModal()">取消</button>
      <button class="btn btn-accent" onclick="startNewDownload()">开始离线下载</button>
    `
  );
}

function startNewDownload(){
  const url = document.getElementById('inpDlUrl')?.value.trim();
  closeModal();
  if(!url){
    toast('请输入下载地址。');
    return;
  }
  toast('当前版本只支持媒体库扫描与云盘导入，离线下载适配器尚未接入，未创建虚假任务。');
}

function clearLogs(){
  const t = document.getElementById('logTerminal');
  if(t) t.innerHTML = '<div class="log-line"><span class="log-time">[本机]</span><span class="log-tag info">INFO</span><span class="log-msg">诊断快照已清空；重新进入本页可读取当前运行时状态。</span></div>';
  toast('系统日志已清空');
}
async function refreshRuntimeDiagnostics(){
  const terminal = document.getElementById('logTerminal');
  if(!terminal) return;
  if(!TtvBackend.available()){
    terminal.innerHTML = '<div class="log-line"><span class="log-time">[浏览器]</span><span class="log-tag info">INFO</span><span class="log-msg">当前页面未连接 Tauri 桌面运行时，无法读取硬件诊断。</span></div>';
    return;
  }
  try{
    const diagnostics = await TtvBackend.invoke('runtime_diagnostics');
    const rows = [
      ['运行时', diagnostics?.playbackAvailable ? 'libmpv 播放内核可用' : '播放内核不可用', diagnostics?.playbackAvailable ? 'info' : 'error'],
      ['增强', diagnostics?.enhancementAvailable ? '检测到可用增强资源' : '未检测到增强资源', diagnostics?.enhancementAvailable ? 'info' : 'warn'],
      ...((Array.isArray(diagnostics?.warnings) ? diagnostics.warnings : []).map(message => ['警告', message, 'warn'])),
      ...((Array.isArray(diagnostics?.errors) ? diagnostics.errors : []).map(message => ['错误', message, 'error']))
    ];
    terminal.innerHTML = rows.map(([time, message, tag]) => `<div class="log-line"><span class="log-time">[${escapeHtml(time)}]</span><span class="log-tag ${tag}">${tag.toUpperCase()}</span><span class="log-msg">${escapeHtml(String(message))}</span></div>`).join('') || '<div class="log-line"><span class="log-time">[运行时]</span><span class="log-tag info">INFO</span><span class="log-msg">未返回诊断事件。</span></div>';
  }catch(error){
    terminal.innerHTML = `<div class="log-line"><span class="log-time">[错误]</span><span class="log-tag error">ERROR</span><span class="log-msg">无法读取桌面运行时诊断：${escapeHtml(backendErrorMessage(error))}</span></div>`;
  }
}
function exportLogs(){
  const text = document.getElementById('logTerminal')?.innerText || '';
  if(!text.trim()){ toast('当前没有可导出的日志。'); return; }
  const blob = new Blob([text], {type:'text/plain;charset=utf-8'});
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = `ttv-cinema-log-${new Date().toISOString().replace(/[:.]/g, '-')}.txt`;
  link.click();
  URL.revokeObjectURL(url);
  toast('日志文件已下载。');
}

/* ============ 深夜档 · 18+ 隔离区（隐藏入口：连续点击 Logo 6 次） ============ */
const adultState = {
  search: '',
  sort: 'release',
  tag: 'all',
  actor: 'all',
  studio: 'all',
  // 分页：上万条 18+ 条目一次性建卡会让界面卡死，每页只渲染 120 张。
  page: 0,
  pageSize: 120,
  returnView: 'home',
  transitioning: false
};

function javPayload(m){
  const payload = m && m.record && m.record.payload && typeof m.record.payload === 'object' ? m.record.payload : {};
  return payload.jav && typeof payload.jav === 'object' ? payload.jav : {};
}
function adultCode(m){
  const jav = javPayload(m);
  const externalId = m && m.record && m.record.payload ? String(m.record.payload.externalId || '') : '';
  return String(jav.code || externalId || '').trim().toUpperCase();
}
function isAdultScraped(m){
  return m.scrapedBy === 'jav' || Boolean(m.record && m.record.payload && m.record.payload.scrapedBy === 'jav');
}
function adultMovies(){ return MOVIES.filter(m => m.adult); }
function adultReleaseDate(m){
  const javDate = String(javPayload(m).releaseDate || '');
  if(/^\d{4}-\d{2}-\d{2}/.test(javDate)) return javDate.slice(0, 10);
  const year = String(m.y || '');
  return /^\d{4}$/.test(year) ? year : '';
}
function adultDurationMin(m){
  const fileSeconds = Number(m.durationSeconds);
  if(Number.isFinite(fileSeconds) && fileSeconds > 0 && fileSeconds !== 3600){
    return Math.round(fileSeconds / 60);
  }
  const minutes = Number(javPayload(m).durationMin);
  if(minutes > 0 && !isPlaceholderDurationMin(minutes)) return minutes;
  return 0;
}
function isPlausibleAdultName(name){
  const text = String(name || '').replace(/\s+/g, ' ').trim();
  if(!text || [...text].length > 40) return false;
  if(/function\s*\(|adsby|javascript|magnet|画像を拡大|磁力|window\.|addClass|jQuery|document\.|var\s+\w+\s*=/i.test(text)) return false;
  if(/[{};=]/.test(text)) return false;
  return true;
}
const ZH_TW_TO_CN = {
  '無碼':'无码','有碼':'有码','中文字幕':'中文字幕','無碼破解':'无码破解','人氣':'人气','人妻':'人妻',
  '癡女':'痴女','癡漢':'痴汉','女優':'女优','單體作品':'单体作品','單體':'单体','數位馬賽克':'数字马赛克',
  '馬賽克破解':'马赛克破解','高畫質':'高画质','畫質':'画质','錄影':'录像','紀錄片':'纪录片',
  '顏射':'颜射','口爆':'口爆','內射':'内射','中出':'中出','潮吹':'潮吹','巨乳':'巨乳','美腿':'美腿',
  '美尻':'美尻','口交':'口交','後入':'后入','騎乘':'骑乘','騎乗位':'骑乘位','後背位':'后入',
  '熟女':'熟女','少女':'少女','學生':'学生','女教師':'女教师','秘書':'秘书','護士':'护士',
  '看護':'看护','風俗':'风俗','泡泡浴':'泡泡浴','按摩':'按摩','偷拍':'偷拍','自拍':'自拍',
  '獨家':'独家','專賣':'专卖','專屬':'专属','發行':'发行','製作':'制作','導演':'导演',
  '系列':'系列','時長':'时长','分鐘':'分钟','標題':'标题','簡介':'简介'
};
const ZH_TW_CHAR = {
  '無':'无','碼':'码','氣':'气','癡':'痴','優':'优','體':'体','數':'数','畫':'画','質':'质',
  '錄':'录','紀':'纪','顏':'颜','內':'内','後':'后','騎':'骑','乗':'乘','學':'学','師':'师',
  '護':'护','風':'风','獨':'独','專':'专','賣':'卖','屬':'属','發':'发','製':'制','導':'导',
  '時':'时','鐘':'钟','標':'标','題':'题','簡':'简','介':'介','臺':'台','灣':'湾','國':'国',
  '與':'与','對':'对','開':'开','關':'关','門':'门','東':'东','來':'来','這':'这','還':'还',
  '裡':'里','麼':'么','個':'个','們':'们','為':'为','會':'会','說':'说','讓':'让','從':'从',
  '當':'当','將':'将','於':'于','並':'并','種':'种','點':'点','長':'长','樣':'样','線':'线'
};
function toSimplifiedZh(value){
  const text = String(value || '');
  if(!text) return '';
  let out = text;
  for(const [from, to] of Object.entries(ZH_TW_TO_CN)){
    if(out.includes(from)) out = out.split(from).join(to);
  }
  out = out.replace(/[\u3400-\u9fff]/g, ch => ZH_TW_CHAR[ch] || ch);
  return out;
}
function adultActors(m){
  const actors = javPayload(m).actors;
  return Array.isArray(actors) ? actors.map(value => toSimplifiedZh(value)).filter(isPlausibleAdultName) : [];
}
function adultStudio(m){
  const studio = toSimplifiedZh(String(javPayload(m).studio || '').trim());
  return isPlausibleAdultName(studio) ? studio : '';
}
function adultTags(m){
  const jav = javPayload(m);
  const raw = Array.isArray(jav.tags) && jav.tags.length
    ? jav.tags.map(String)
    : (Array.isArray(m.genres) ? m.genres.filter(g => g && g !== '成人' && g !== '未分类') : []);
  return raw.map(value => toSimplifiedZh(value)).filter(isPlausibleAdultName);
}
function adultDirector(m){ return toSimplifiedZh(String(javPayload(m).director || '').trim()); }
function adultLabel(m){ return toSimplifiedZh(String(javPayload(m).label || '').trim()); }
function adultSeries(m){ return toSimplifiedZh(String(javPayload(m).series || '').trim()); }
function adultQualityLabel(m){
  return detectMediaQualityLabel(m?.sourceTitle || m?.t || m?.record?.remotePath) || '';
}
function adultProviderLabel(m){
  const provider = String(javPayload(m).provider || '').trim();
  return provider || (isAdultScraped(m) ? 'jav' : '');
}
function isPlaceholderSummary(text){
  return !String(text || '').trim() || /已从(媒体中心|本地目录)导入|暂无简介|该条目没有绑定/.test(String(text));
}
function composeAdultSummary(m){
  const plot = toSimplifiedZh(String(javPayload(m).summary || '').trim());
  if(plot && !isPlaceholderSummary(plot)) return plot;
  if(m?.summary && !isPlaceholderSummary(m.summary)) return toSimplifiedZh(m.summary);
  const parts = [];
  if(m?.t) parts.push(toSimplifiedZh(m.t));
  const facts = [];
  if(adultCode(m)) facts.push('番号 ' + adultCode(m));
  if(adultStudio(m)) facts.push('制作 ' + adultStudio(m));
  if(adultLabel(m)) facts.push('发行 ' + adultLabel(m));
  if(adultSeries(m)) facts.push('系列 ' + adultSeries(m));
  if(adultReleaseDate(m)) facts.push('发行日 ' + adultReleaseDate(m));
  if(adultDurationMin(m)) facts.push('时长 ' + adultDurationMin(m) + ' 分钟');
  if(adultActors(m).length) facts.push('出演 ' + adultActors(m).join('、'));
  if(adultTags(m).length) facts.push('标签 ' + adultTags(m).slice(0, 8).join('、'));
  if(facts.length) parts.push(facts.join(' · '));
  return parts.join('。') || '暂无简介。';
}
function actorWorkCount(name){
  return adultMovies().filter(m => adultActors(m).includes(name)).length;
}
function renderAdultActors(m){
  const section = document.getElementById('dAdultActors');
  const row = document.getElementById('dAdultActorRow');
  if(!section || !row) return;
  const actors = adultActors(m);
  if(!m?.adult || !actors.length){
    section.hidden = true;
    row.innerHTML = '';
    return;
  }
  row.innerHTML = actors.map(actor => {
    const count = actorWorkCount(actor);
    return `<button type="button" class="adult-actor-card" data-adult-actor="${escapeHtml(actor)}"><b>${escapeHtml(actor)}</b><span>${count} 部作品</span></button>`;
  }).join('');
  row.querySelectorAll('[data-adult-actor]').forEach(btn => btn.addEventListener('click', () => {
    openAdultActorWorks(btn.dataset.adultActor);
  }));
  section.hidden = false;
}
function openAdultActorWorks(actor){
  if(!actor) return;
  adultState.actor = actor;
  adultState.search = '';
  adultState.page = 0;
  const searchInput = document.getElementById('adultSearch');
  if(searchInput) searchInput.value = '';
  void enterAdultZone();
  renderAdultZone();
  toast('已筛选演员：' + actor);
}
function renderAdultDetailFacts(m){
  const section = document.getElementById('dAdultFacts');
  const grid = document.getElementById('dAdultFactGrid');
  const tags = document.getElementById('dAdultTags');
  if(!section || !grid) return;
  if(!m?.adult){
    section.hidden = true;
    grid.innerHTML = '';
    if(tags) tags.innerHTML = '';
    return;
  }
  const facts = [
    ['番号', adultCode(m)],
    ['发行日', adultReleaseDate(m)],
    ['时长', adultDurationMin(m) ? adultDurationMin(m) + ' 分钟' : ''],
    ['制作商', adultStudio(m)],
    ['发行商', adultLabel(m)],
    ['导演', adultDirector(m)],
    ['系列', adultSeries(m)],
    ['评分', m.r ? Number(m.r).toFixed(1) : ''],
    ['清晰度', adultQualityLabel(m)],
    ['数据源', adultProviderLabel(m)]
  ].filter(([, value]) => value);
  grid.innerHTML = facts.map(([label, value]) =>
    `<div class="adult-fact"><span>${escapeHtml(label)}</span><b>${escapeHtml(String(value))}</b></div>`
  ).join('');
  if(tags){
    tags.innerHTML = adultTags(m).slice(0, 16).map(tag =>
      `<span class="adult-tag-chip">${escapeHtml(tag)}</span>`
    ).join('');
  }
  section.hidden = !facts.length && !(tags && tags.innerHTML);
}

function compactAdultToken(value){
  return String(value || '').toUpperCase().replace(/[^A-Z0-9\u4e00-\u9fff]/g, '');
}
function adultSearchHaystack(m){
  const jav = javPayload(m);
  const aliases = Array.isArray(jav.aliases) ? jav.aliases : [];
  const codes = Array.isArray(jav.codes) ? jav.codes : [];
  return [
    m.t,
    m.sourceTitle,
    jav.title,
    adultCode(m),
    adultStudio(m),
    adultLabel(m),
    adultSeries(m),
    adultDirector(m),
    adultReleaseDate(m),
    m.y,
    m.summary,
    jav.summary,
    ...aliases,
    ...codes,
    ...adultActors(m),
    ...adultTags(m)
  ].filter(Boolean).join('\n').toLowerCase();
}
function adultMatchesQuery(m, raw){
  const query = String(raw || '').trim();
  if(!query) return true;
  const haystack = adultSearchHaystack(m);
  const compactHay = compactAdultToken(haystack);
  const compactCodes = compactAdultToken([
    adultCode(m),
    m.sourceTitle,
    ...(Array.isArray(javPayload(m).codes) ? javPayload(m).codes : [])
  ].filter(Boolean).join(' '));
  const tokens = query.toLowerCase().split(/[\s,，、|/+]+/).map(token => token.trim()).filter(Boolean);
  return tokens.every(token => {
    if(haystack.includes(token)) return true;
    const compact = compactAdultToken(token);
    if(compact.length >= 2 && (compactCodes.includes(compact) || compactHay.includes(compact))) return true;
    return false;
  });
}
function adultVisibleMovies(){
  const q = adultState.search.trim();
  const list = adultMovies().filter(m => {
    if(adultState.tag !== 'all' && !adultTags(m).includes(adultState.tag)) return false;
    if(adultState.actor !== 'all' && !adultActors(m).includes(adultState.actor)) return false;
    if(adultState.studio !== 'all' && adultStudio(m) !== adultState.studio) return false;
    return adultMatchesQuery(m, q);
  });
  if(adultState.sort === 'title'){
    list.sort((a, b) => a.t.localeCompare(b.t, 'zh-CN'));
  } else if(adultState.sort === 'duration'){
    list.sort((a, b) => adultDurationMin(b) - adultDurationMin(a));
  } else if(adultState.sort === 'added'){
    // MOVIES 按入库顺序排列，倒序即最新入库优先
    list.reverse();
  } else {
    list.sort((a, b) => (adultReleaseDate(b) || '').localeCompare(adultReleaseDate(a) || ''));
  }
  return list;
}

function renderAdultFilters(){
  const box = document.getElementById('adultFilters');
  if(!box) return;
  const all = adultMovies();
  const countBy = getter => {
    const map = new Map();
    all.forEach(m => getter(m).forEach(value => { if(value) map.set(value, (map.get(value) || 0) + 1); }));
    return [...map.entries()].sort((a, b) => b[1] - a[1]);
  };
  const tags = countBy(m => adultTags(m)).slice(0, 24);
  const actors = countBy(m => adultActors(m)).slice(0, 20);
  const studios = countBy(m => { const s = adultStudio(m); return s ? [s] : []; }).slice(0, 16);
  const row = (label, entries, key, allLabel) => {
    if(!entries.length) return '';
    const chips = entries.map(([value, count]) =>
      `<button class="pill adult-filter-pill${adultState[key] === value ? ' active' : ''}" data-adult-filter="${key}" data-adult-value="${escapeHtml(value)}">${escapeHtml(value)}<small>${count}</small></button>`
    ).join('');
    return `<div class="adult-filter-row"><span class="adult-filter-label">${label}</span><div class="adult-filter-chips"><button class="pill adult-filter-pill${adultState[key] === 'all' ? ' active' : ''}" data-adult-filter="${key}" data-adult-value="all">${allLabel}</button>${chips}</div></div>`;
  };
  const html = row('标签', tags, 'tag', '全部标签') + row('演员', actors, 'actor', '全部演员') + row('厂商', studios, 'studio', '全部厂商');
  box.innerHTML = html || '<div class="adult-filter-empty">刮削完成后，这里会显示标签、演员与厂商分类。</div>';
  box.querySelectorAll('[data-adult-filter]').forEach(btn => btn.addEventListener('click', () => {
    adultState[btn.dataset.adultFilter] = btn.dataset.adultValue;
    adultState.page = 0;
    renderAdultFilters();
    renderAdultGrid();
  }));
  // 标签行横向溢出：滚轮转为横向滚动，否则鼠标用户滚不到右侧被截断的标签。
  box.querySelectorAll('.adult-filter-chips').forEach(strip => {
    strip.addEventListener('wheel', event => {
      if(Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
      if(strip.scrollWidth <= strip.clientWidth) return;
      event.preventDefault();
      strip.scrollLeft += event.deltaY;
    }, {passive:false});
  });
}

// Media ids whose first-frame cover is already captured or currently being
// captured; prevents duplicate FFmpeg work across re-renders and retries.
const adultFirstFrameDone = new Set();
let adultFirstFrameObserver = null;
function adultCssEscape(value){
  return (window.CSS && typeof window.CSS.escape === 'function') ? window.CSS.escape(String(value)) : String(value);
}

// Capture the video's first frame and store it as the item's cover. Used for
// 18+ items that have no JAV metadata (and therefore no scraped cover). The
// backend resolves the provider playback URL and runs the bundled FFmpeg.
async function applyAdultFirstFrame(m, imgEl){
  if(!TtvBackend.available()) return;
  const id = String(m.id);
  if(adultFirstFrameDone.has(id)) return;
  adultFirstFrameDone.add(id);
  try{
    const path = await TtvBackend.invoke('adult_first_frame_cover', {input: {mediaId: id, force: false}});
    if(!path){ adultFirstFrameDone.delete(id); return; }
    const url = normalizeArtworkUrl(String(path));
    if(m.record) m.record.artUrl = String(path);
    m.img = url;
    m.hasArtwork = true;
    // Update whichever card is currently in the DOM for this movie (the grid
    // may have been rebuilt while the capture was in flight).
    const currentImg = document.querySelector(`.adult-card[data-movie-id="${adultCssEscape(id)}"] img`) || imgEl;
    if(currentImg){ delete currentImg.dataset.artFallback; currentImg.src = url; }
  }catch(error){
    adultFirstFrameDone.delete(id);
    console.warn('Unable to capture first-frame cover:', error);
  }
}

// Lazily trigger a first-frame capture once a no-cover card scrolls near the
// viewport, so we do not spawn FFmpeg for hundreds of off-screen items at once.
function observeAdultFirstFrame(cardEl, m, imgEl){
  cardEl.__adultMovie = m;
  cardEl.__adultImg = imgEl;
  if(!('IntersectionObserver' in window)){ void applyAdultFirstFrame(m, imgEl); return; }
  if(!adultFirstFrameObserver){
    adultFirstFrameObserver = new IntersectionObserver(entries => {
      entries.forEach(entry => {
        if(!entry.isIntersecting) return;
        const target = entry.target;
        adultFirstFrameObserver.unobserve(target);
        void applyAdultFirstFrame(target.__adultMovie, target.__adultImg);
      });
    }, { rootMargin: '400px 0px' });
  }
  adultFirstFrameObserver.observe(cardEl);
}

async function retryAdultCover(m, imgEl){
  if(!TtvBackend.available() || !imgEl || imgEl.dataset.adultCoverRetry === '1') return;
  imgEl.dataset.adultCoverRetry = '1';
  // JAV-scraped items keep the dedicated cover download path (JavBus/Avmoo).
  if(Boolean(javPayload(m).code)){
    try{
      const path = await TtvBackend.invoke('adult_cover_fetch', {input: {mediaId: String(m.id), force: false}});
      if(path){
        const url = normalizeArtworkUrl(String(path));
        if(m.record) m.record.artUrl = String(path);
        m.img = url;
        m.hasArtwork = true;
        delete imgEl.dataset.artFallback;
        imgEl.src = url;
        return;
      }
    }catch(error){
      console.warn('JAV cover fetch failed, falling back to first frame:', error);
    }
  }
  // No JAV metadata (or its cover is unavailable): use the video's first frame.
  await applyAdultFirstFrame(m, imgEl);
}

function buildAdultCard(m, index){
  const code = adultCode(m);
  const scraped = isAdultScraped(m);
  const duration = adultDurationMin(m);
  const release = adultReleaseDate(m);
  const actors = adultActors(m);
  const quality = adultQualityLabel(m);
  const title = toSimplifiedZh(m.t);
  const el = document.createElement('div');
  el.className = 'adult-card';
  el.dataset.movieId = String(m.id ?? '');
  const actorChips = actors.slice(0, 3).map(actor =>
    `<span class="adult-actor-chip" data-adult-actor="${escapeHtml(actor)}" title="筛选演员：${escapeHtml(actor)}">${escapeHtml(actor)}</span>`
  ).join('');
  el.innerHTML = `
    <div class="adult-cover-wrap">
      <img alt="${escapeHtml(title)}" loading="lazy"/>
      <span class="adult-code-badge">${escapeHtml(code || '未知番号')}</span>
      ${quality ? `<span class="adult-quality-badge">${escapeHtml(quality)}</span>` : ''}
      ${duration ? `<span class="adult-duration-badge">${duration} 分钟</span>` : ''}
      ${scraped ? '' : '<span class="adult-unscraped-badge">未刮削</span>'}
      <div class="adult-play-overlay"><span class="adult-play-btn"><svg viewBox="0 0 24 24" fill="#fff"><path d="M8 5.5v13l11-6.5z"/></svg></span></div>
    </div>
    <div class="adult-card-meta">
      <div class="adult-card-title" title="${escapeHtml(title)}">${escapeHtml(title)}</div>
      <div class="adult-card-sub">${escapeHtml(code || '—')}${release ? ' · ' + escapeHtml(release) : ''}${duration ? ' · ' + duration + ' 分钟' : ''}${quality ? ' · ' + escapeHtml(quality) : ''}</div>
      ${actorChips ? `<div class="adult-card-actors">${actorChips}</div>` : ''}
      ${adultStudio(m) ? `<div class="adult-card-sub">${escapeHtml(adultStudio(m))}${adultSeries(m) ? ' · ' + escapeHtml(adultSeries(m)) : ''}</div>` : ''}
    </div>`;
  const img = el.querySelector('img');
  img.dataset.coverSrc = normalizeArtworkUrl(m.img || m.artRemote || '', '/assets/detail-poster.jpg');
  bindCardCover(img, {eager: index < COVER_EAGER_COUNT, owner: m});
  // 封面缺失时交给后端按需下载（JavBus/Avmoo 原图）
  img.addEventListener('error', () => { void retryAdultCover(m, img); });
  // 无元数据且无封面的条目：滚动进入视野时抓取视频首帧作为封面
  if(!scraped && !m.hasArtwork && !adultFirstFrameDone.has(String(m.id))){
    observeAdultFirstFrame(el, m, img);
  }
  el.addEventListener('click', () => openDetail(m, el));
  el.querySelectorAll('[data-adult-actor]').forEach(chip => chip.addEventListener('click', e => {
    e.stopPropagation();
    adultState.actor = chip.dataset.adultActor;
    renderAdultFilters();
    renderAdultGrid();
  }));
  return el;
}

function renderAdultGrid(){
  const grid = document.getElementById('adultGrid');
  if(!grid) return;
  // Drop stale observations; qualifying cards re-register as they are rebuilt.
  if(adultFirstFrameObserver) adultFirstFrameObserver.disconnect();
  const visible = adultVisibleMovies();
  grid.innerHTML = '';
  if(!visible.length){
    grid.innerHTML = adultMovies().length
      ? '<p class="catalog-empty">没有匹配的作品，试试其他关键词或清空筛选条件。</p>'
      : '<p class="catalog-empty">深夜档还没有内容。导入文件名带番号（如 ABC-123）的视频后，会自动归类到这里。</p>';
    return;
  }
  // 分页渲染：筛选/搜索/排序变化时把页码夹回有效范围，避免越界白屏。
  const pageCount = Math.max(1, Math.ceil(visible.length / adultState.pageSize));
  if(adultState.page >= pageCount) adultState.page = pageCount - 1;
  if(adultState.page < 0) adultState.page = 0;
  const start = adultState.page * adultState.pageSize;
  const pageItems = visible.slice(start, start + adultState.pageSize);
  const fragment = document.createDocumentFragment();
  pageItems.forEach((m, i) => fragment.appendChild(buildAdultCard(m, i)));
  grid.appendChild(fragment);
  // 翻页控件：单页时不出现在 DOM，不打扰布局。
  if(pageCount > 1){
    const nav = document.createElement('div');
    nav.className = 'adult-pagination';
    const windowStart = start + 1;
    const windowEnd = Math.min(visible.length, start + adultState.pageSize);
    nav.innerHTML = `
      <button type="button" class="btn btn-ghost adult-page-btn" data-adult-page="prev" ${adultState.page === 0 ? 'disabled' : ''}>← 上一页</button>
      <span class="adult-page-info">第 ${adultState.page + 1} / ${pageCount} 页 · ${windowStart}-${windowEnd} / 共 ${visible.length} 部</span>
      <button type="button" class="btn btn-ghost adult-page-btn" data-adult-page="next" ${adultState.page >= pageCount - 1 ? 'disabled' : ''}>下一页 →</button>
    `;
    nav.querySelectorAll('[data-adult-page]').forEach(btn => btn.addEventListener('click', () => {
      const delta = btn.dataset.adultPage === 'prev' ? -1 : 1;
      const nextPage = Math.min(pageCount - 1, Math.max(0, adultState.page + delta));
      if(nextPage === adultState.page) return;
      adultState.page = nextPage;
      renderAdultGrid();
      // 翻页后回到列表顶部，保持阅读位置一致。
      grid.scrollIntoView({behavior: 'instant', block: 'start'});
    }));
    grid.appendChild(nav);
  }
}

function renderAdultZone(){
  const all = adultMovies();
  const scraped = all.filter(isAdultScraped).length;
  const setText = (id, value) => { const el = document.getElementById(id); if(el) el.textContent = String(value); };
  // 数据未就绪（应用刚启动、全量库还在分页加载）时不渲染假 0 统计，
  // 显示加载占位；applyCatalog 完成后会因 currentView==='adult' 自动重渲染。
  if(!libraryDataReady && !all.length){
    setText('adultStatTotal', '…');
    setText('adultStatScraped', '…');
    setText('adultStatMissing', '…');
    const grid = document.getElementById('adultGrid');
    if(grid) grid.innerHTML = '<p class="catalog-empty">正在加载媒体库，几秒后自动刷新…</p>';
    const filters = document.getElementById('adultFilters');
    if(filters) filters.innerHTML = '';
    return;
  }
  setText('adultStatTotal', all.length);
  setText('adultStatScraped', scraped);
  setText('adultStatMissing', all.length - scraped);
  const searchInput = document.getElementById('adultSearch');
  if(searchInput && !searchInput.dataset.adultBound){
    searchInput.dataset.adultBound = '1';
    searchInput.addEventListener('input', () => { adultState.search = searchInput.value; adultState.page = 0; renderAdultGrid(); });
  }
  const sortSelect = document.getElementById('adultSort');
  if(sortSelect && !sortSelect.dataset.adultBound){
    sortSelect.dataset.adultBound = '1';
    sortSelect.addEventListener('change', () => { adultState.sort = sortSelect.value; adultState.page = 0; renderAdultGrid(); });
  }
  renderAdultFilters();
  renderAdultGrid();
}

async function scrapeMissingAdult(){
  const button = document.getElementById('adultScrapeBtn');
  if(!TtvBackend.available()){ toast('当前页面未连接桌面端。'); return; }
  const missing = adultMovies().filter(m => !isAdultScraped(m) || !adultActors(m).length || !m.r || isPlaceholderSummary(m.summary));
  if(!missing.length){ toast('深夜档没有待刮削的条目。'); return; }
  if(!resetScanProgress('scrape', '深夜档刮削缺失元数据')) return;
  if(button){ button.disabled = true; button.classList.add('loading'); }
  logScanProgress(`开始刮削 ${missing.length} 条缺失条目（JavBus / JavDB / Avmoo / JavLibrary / Jav321）。`);
  updateScanProgress(`使用 JavBus / JavDB / Avmoo / JavLibrary / Jav321 匹配媒体`, null);
  scanProgress.files = missing.length;
  scanProgress.phase = 'scrape';
  scanProgress.resumeOnLaunch = true;
  scanProgress.scrapeOptions = {mode:'adult-missing', overwrite:true, javScope:'full'};
  saveScanTasks();
  toast(`开始刮削 ${missing.length} 条缺失元数据（JavBus / JavDB / Avmoo / JavLibrary / Jav321）…`);
  try{
    const report = await invokePersistentLibraryScrape({
      mediaIds: missing.map(m => String(m.id)),
      providers: ['jav'],
      overwrite: true,
      includeAdult: true,
      limit: Math.max(1, missing.length)
    });
    if(scanTaskPausedByUser()) return;
    await loadInitialCatalog();
    scanProgress.matched = Number(report?.matched || 0);
    scanProgress.updated = Number(report?.updated || 0);
    scanProgress.skipped = Number(report?.unmatched || 0);
    scanProgress.covers = Number(report?.covers || 0);
    scanProgress.adultIsolated = Number(report?.adultIsolated || 0);
    finishScanProgress(report
      ? `深夜档刮削完成：更新 ${report.updated || 0} 条，未匹配 ${report.unmatched || 0} 条。`
      : '刮削未执行，请检查桌面端连接。');
    toast(scanProgress.message);
  }catch(error){
    finishScanProgress('深夜档刮削失败：' + backendErrorMessage(error), true);
    toast(scanProgress.message);
  }finally{
    if(button){ button.disabled = false; button.classList.remove('loading'); }
  }
}

/* ---- 隐藏入口：连续点击 Logo 6 次进入；深夜档内点击 Logo 退出 ---- */
let brandClickCount = 0;
let brandClickTimer = null;
const BRAND_CLICK_WINDOW = 2500;
const BRAND_CLICK_TARGET = 6;

function handleBrandClick(){
  if(currentView === 'adult'){ exitAdultZone(); return; }
  brandClickCount += 1;
  if(brandClickTimer) clearTimeout(brandClickTimer);
  brandClickTimer = setTimeout(() => { brandClickCount = 0; brandClickTimer = null; }, BRAND_CLICK_WINDOW);
  if(brandClickCount >= BRAND_CLICK_TARGET){
    brandClickCount = 0;
    if(brandClickTimer){ clearTimeout(brandClickTimer); brandClickTimer = null; }
    void enterAdultZone();
    return;
  }
  showView('home');
}

function playAdultTransition(duringPeakCallback){
  return new Promise(resolve => {
    const overlay = document.getElementById('adultTransition');
    if(!overlay || reducedMotion){
      if(duringPeakCallback) duringPeakCallback();
      resolve();
      return;
    }
    overlay.classList.remove('playing');
    void overlay.offsetWidth;
    overlay.classList.add('playing');
    // 在全屏暗场 100% 遮蔽的黄金中点 (280ms) 执行视图切换，结合 rAF 确保帧同步
    setTimeout(() => {
      requestAnimationFrame(() => {
        if(duringPeakCallback) duringPeakCallback();
        resolve();
      });
    }, 280);
    // 750ms 播放完毕后再解除遮罩
    setTimeout(() => {
      overlay.classList.remove('playing');
    }, 750);
  });
}

async function enterAdultZone(){
  if(adultState.transitioning || currentView === 'adult') return;
  adultState.transitioning = true;
  adultState.returnView = currentView === 'detail' ? 'home' : currentView;
  // 每次进入都从第一页开始，避免沿用上次会话遗留的页码。
  adultState.page = 0;
  try{
    await playAdultTransition(() => {
      document.body.classList.add('adult-mode');
      showView('adult', false);
    });
    // 过渡回调里 showView 发生时 transitioning 仍为 true；这里补一次，
    // 避免统计停在 HTML 默认的 0、网格整页空白。
    renderAdultZone();
  }finally{
    adultState.transitioning = false;
  }
}

async function exitAdultZone(){
  if(adultState.transitioning || currentView !== 'adult') return;
  adultState.transitioning = true;
  try{
    await playAdultTransition(() => {
      document.body.classList.remove('adult-mode');
      showView(adultState.returnView || 'home', false);
    });
  }finally{
    adultState.transitioning = false;
  }
}

/* 详情页返回：按来源回到深夜档、短剧或影视库。 */
let detailReturnView = 'library';
function detailBack(){
  const target = detailReturnView === 'adult' || detailReturnView === 'short-drama'
    ? detailReturnView
    : 'library';
  detailReturnView = 'library';
  showView(target);
}

/* ============ 糖心影院 · 在线目录（深夜档内嵌，只读浏览 + 显式开映） ============ */
/* 遵循 tangxin-zhizhe-extension 的两段式边界：海报墙阶段只调用只读目录
   接口（discover / search / detail），绝不保存播放地址；用户点击「开映」
   才向后端换取本会话 m3u8 并交给播放器。 */
const adultZoneState = { zone: 'library' };
const txState = {
  mode: 'discover',
  sections: [],
  items: [],
  page: 0,
  hasMore: false,
  loading: false,
  seq: 0,
  discoverLoaded: false,
  detail: null,
  detailLoading: false,
  detailCurrent: null,
  playRequest: 0,
  accounts: [],
  accountsLoading: false,
  confirmBuy: null,
  cloudConfig: null,
  cloudBusy: false
};
const txPosterCache = new Map();
let txPosterObserver = null;

function txPosterEnsureObserver(){
  if(txPosterObserver) return txPosterObserver;
  txPosterObserver = new IntersectionObserver(entries => {
    entries.forEach(entry => {
      if(entry.isIntersecting){
        txPosterObserver.unobserve(entry.target);
        void txLoadPoster(entry.target);
      }
    });
  }, { rootMargin: '360px' });
  return txPosterObserver;
}
function txObservePosters(root){
  root.querySelectorAll('img[data-txposter]').forEach(img => txPosterEnsureObserver().observe(img));
}
async function txLoadPoster(img){
  const url = img.getAttribute('data-txposter');
  if(!url || img.getAttribute('src')) return;
  const cached = txPosterCache.get(url);
  if(cached){ img.src = cached; return; }
  img.removeAttribute('data-txposter');
  try{
    const dataUrl = await TtvBackend.invoke('tangxin_poster', {posterUrl: url});
    if(txPosterCache.size > 160) txPosterCache.delete(txPosterCache.keys().next().value);
    txPosterCache.set(url, dataUrl);
    if(img.isConnected) img.src = dataUrl;
  }catch(error){
    if(img.isConnected) img.classList.add('tx-poster-failed');
  }
}

function txAccessBadge(movie){
  const labels = { free: '', vip: 'VIP', coin: (movie.price ? `金币 ${movie.price}` : '金币') };
  const label = labels[movie.access];
  return label ? `<span class="tx-badge tx-badge-${escapeHtml(movie.access)}">${escapeHtml(label)}</span>` : '';
}
function txCardHtml(movie){
  const ratio = movie.orientation === 'portrait' ? '3/4' : (movie.orientation === 'square' ? '1/1' : '16/10');
  return `<article class="tx-card" data-txid="${escapeHtml(movie.id)}" style="--tx-ratio:${ratio}">
    <div class="tx-card-cover">
      <img alt="" data-txposter="${escapeHtml(movie.posterUrl)}">
      ${txAccessBadge(movie)}
      ${movie.durationLabel ? `<span class="tx-card-duration">${escapeHtml(movie.durationLabel)}</span>` : ''}
    </div>
    <div class="tx-card-meta">
      <b class="tx-card-title">${escapeHtml(movie.title)}</b>
      <span class="tx-card-creator">${escapeHtml(movie.creator)}${movie.views ? ' · ' + escapeHtml(movie.views) + ' 播放' : ''}</span>
    </div>
  </article>`;
}
function txBindCards(root){
  root.querySelectorAll('[data-txid]').forEach(card => card.addEventListener('click', () => {
    const movie = txMovieById(card.dataset.txid);
    if(movie) void openTangxinDetail(movie);
  }));
  txObservePosters(root);
}
function txMovieById(id){
  for(const item of txState.items) if(item.id === id) return item;
  for(const section of txState.sections) for(const item of section.items) if(item.id === id) return item;
  const groups = txState.detail?.groups || [];
  for(const item of groups) if(item.id === id) return item;
  return txState.detailCurrent && txState.detailCurrent.id === id ? txState.detailCurrent : null;
}
function txSetStatus(message, isError = false){
  const el = document.getElementById('txStatus');
  if(!el) return;
  el.hidden = !message;
  el.textContent = message || '';
  el.classList.toggle('error', Boolean(isError));
}
function txRenderDiscover(){
  const holder = document.getElementById('txSections');
  if(!holder) return;
  holder.innerHTML = txState.sections.map(section =>
    `<section class="tx-section">
      <h3 class="tx-section-title">${escapeHtml(section.name)}</h3>
      <div class="tx-rail">${section.items.map(txCardHtml).join('')}</div>
    </section>`
  ).join('') || '<p class="catalog-empty">本期没有可展示的推荐内容。</p>';
  txBindCards(holder);
}
function txRenderGrid(){
  const grid = document.getElementById('txGrid');
  if(!grid) return;
  grid.innerHTML = txState.items.map(txCardHtml).join('') || '<p class="catalog-empty">没有匹配的影片。</p>';
  txBindCards(grid);
  const moreRow = document.getElementById('txMoreRow');
  if(moreRow){
    moreRow.hidden = !(txState.mode === 'search' && txState.hasMore && !txState.loading);
  }
}
async function loadTangxinDiscover(){
  if(txState.loading) return;
  if(!TtvBackend.available()){ txSetStatus('糖心影院需要桌面端连接（当前页面未连接 TTV 后端）。', true); return; }
  txState.loading = true;
  txSetStatus('正在连接糖心影院…');
  try{
    const sections = await TtvBackend.invoke('tangxin_discover');
    txState.sections = Array.isArray(sections) ? sections : [];
    txState.discoverLoaded = true;
    txState.mode = 'discover';
    txState.items = [];
    txSetStatus('');
    txRenderDiscover();
    const grid = document.getElementById('txGrid');
    if(grid) grid.innerHTML = '';
    const moreRow = document.getElementById('txMoreRow');
    if(moreRow) moreRow.hidden = true;
  }catch(error){
    txSetStatus('糖心影院加载失败：' + backendErrorMessage(error), true);
  }finally{
    txState.loading = false;
  }
}
async function runTangxinSearch(reset = false){
  if(txState.loading) return;
  const query = (document.getElementById('txSearchInput')?.value || '').trim();
  const orderSelect = document.getElementById('txOrderSelect');
  const paySelect = document.getElementById('txPaySelect');
  const canvasSelect = document.getElementById('txCanvasSelect');
  txState.loading = true;
  if(reset) txState.page = 1; else txState.page += 1;
  const seq = ++txState.seq;
  if(reset){
    txState.items = [];
    txSetStatus('正在搜索…');
    txRenderGrid();
  }
  try{
    const result = await TtvBackend.invoke('tangxin_search', {request:{
      keywords: query,
      order: orderSelect?.value || 'new',
      payType: paySelect?.value || '',
      canvas: canvasSelect?.value || '',
      page: txState.page,
      pageSize: 24
    }});
    if(seq !== txState.seq) return; // 旧响应不覆盖新查询
    txState.mode = 'search';
    const incoming = Array.isArray(result?.items) ? result.items : [];
    const seen = new Set(txState.items.map(item => item.id));
    incoming.forEach(item => { if(!seen.has(item.id)){ seen.add(item.id); txState.items.push(item); } });
    txState.hasMore = Boolean(result?.hasMore);
    txSetStatus('');
    const holder = document.getElementById('txSections');
    if(holder) holder.innerHTML = '';
    txRenderGrid();
  }catch(error){
    if(seq === txState.seq) txSetStatus('糖心搜索失败：' + backendErrorMessage(error), true);
  }finally{
    if(seq === txState.seq) txState.loading = false;
  }
}
function switchAdultZone(zone){
  adultZoneState.zone = zone === 'cinema' ? 'cinema' : 'library';
  document.querySelectorAll('#view-adult .zone-btn').forEach(btn =>
    btn.classList.toggle('active', btn.dataset.adultZone === adultZoneState.zone));
  const cinema = document.getElementById('txCinema');
  const cinemaOn = adultZoneState.zone === 'cinema';
  if(cinema) cinema.hidden = !cinemaOn;
  const toolbar = document.querySelector('#view-adult .adult-toolbar-library');
  const filters = document.getElementById('adultFilters');
  const grid = document.getElementById('adultGrid');
  if(toolbar) toolbar.hidden = cinemaOn;
  if(filters) filters.hidden = cinemaOn;
  if(grid) grid.hidden = cinemaOn;
  if(cinemaOn){
    if(!txState.discoverLoaded) void loadTangxinDiscover();
    else { txRenderDiscover(); if(txState.mode === 'search') txRenderGrid(); }
  }
}
function openTangxinZoneFromCinema(){
  const input = document.getElementById('txSearchInput');
  if(input) input.value = '';
  txState.mode = 'discover';
  txState.items = [];
  txRenderDiscover();
  const grid = document.getElementById('txGrid');
  if(grid) grid.innerHTML = '';
  const moreRow = document.getElementById('txMoreRow');
  if(moreRow) moreRow.hidden = true;
  if(!txState.discoverLoaded) void loadTangxinDiscover();
}

async function openTangxinDetail(movie){
  if(txState.detailLoading) return;
  if(!TtvBackend.available()){ toast('糖心影院需要桌面端连接。'); return; }
  const overlay = document.getElementById('txDetailOverlay');
  if(!overlay) return;
  txState.detail = null;
  txState.detailCurrent = movie;
  txState.detailLoading = true;
  txState.confirmBuy = null;
  const playHint = document.getElementById('txPlayHint');
  if(playHint) playHint.textContent = '';
  overlay.hidden = false;
  const setTitle = text => { const el = document.getElementById('txDetailTitle'); if(el) el.textContent = text; };
  setTitle('正在加载详情…');
  try{
    const detail = await TtvBackend.invoke('tangxin_detail', {movieId: String(movie.id)});
    if(!detail?.movie) throw new Error('详情响应无效');
    txState.detail = detail;
    txState.detailCurrent = detail.movie;
    const groups = Array.isArray(detail.groups) ? detail.groups : [];
    const meta = [
      detail.movie.creator,
      detail.movie.durationLabel,
      detail.movie.access === 'vip' ? 'VIP' : (detail.movie.access === 'coin' ? `金币 ${detail.movie.price || ''}`.trim() : '免费'),
      detail.movie.publishedAt,
      detail.movie.views ? detail.movie.views + ' 播放' : '',
      detail.movie.score ? '评分 ' + detail.movie.score : '',
      detail.catName
    ].filter(Boolean).join(' · ');
    setTitle(detail.movie.title);
    const metaEl = document.getElementById('txDetailMeta');
    if(metaEl) metaEl.textContent = meta;
    const descEl = document.getElementById('txDetailDesc');
    if(descEl) descEl.textContent = detail.description || '暂无简介。';
    const tagsEl = document.getElementById('txDetailTags');
    if(tagsEl) tagsEl.innerHTML = (detail.tags || []).slice(0, 16).map(tag =>
      `<span class="adult-tag-chip">${escapeHtml(tag)}</span>`).join('');
    const poster = document.getElementById('txDetailPoster');
    if(poster){
      poster.removeAttribute('src');
      poster.classList.remove('tx-poster-failed');
      const cached = txPosterCache.get(detail.movie.posterUrl);
      if(cached) poster.src = cached;
      else{
        poster.setAttribute('data-txposter', detail.movie.posterUrl);
        txPosterEnsureObserver().observe(poster);
      }
    }
    const groupsWrap = document.getElementById('txDetailGroups');
    const groupList = document.getElementById('txGroupList');
    const playable = groups.filter(item => item.id !== detail.movie.id || groups.length === 1);
    if(groupsWrap && groupList){
      const showGroups = playable.length > 1;
      groupsWrap.hidden = !showGroups;
      groupList.innerHTML = showGroups ? playable.map(item =>
        `<button type="button" class="tx-group-chip${item.id === detail.movie.id ? ' active' : ''}" data-txgroup="${escapeHtml(item.id)}">${escapeHtml(item.title)}${item.durationLabel ? '（' + escapeHtml(item.durationLabel) + '）' : ''}</button>`
      ).join('') : '';
      groupList.querySelectorAll('[data-txgroup]').forEach(chip => chip.addEventListener('click', () => {
        const next = txMovieById(chip.dataset.txgroup);
        if(!next) return;
        txState.detailCurrent = next;
        groupList.querySelectorAll('.tx-group-chip').forEach(c => c.classList.toggle('active', c.dataset.txgroup === next.id));
      }));
    }
  }catch(error){
    overlay.hidden = true;
    toast('糖心详情加载失败：' + backendErrorMessage(error));
  }finally{
    txState.detailLoading = false;
  }
}
function closeTangxinDetail(){
  const overlay = document.getElementById('txDetailOverlay');
  if(overlay) overlay.hidden = true;
  txState.detail = null;
  txState.detailCurrent = null;
  txState.confirmBuy = null;
  const hint = document.getElementById('txPlayHint');
  if(hint) hint.textContent = '';
}

/* ---- 糖心账号池：账号密码 / token 凭证（含群内共享导入）/ 二维码凭证 ---- */
async function txLoadCloudConfig(){
  if(!TtvBackend.available()) return;
  try{
    txState.cloudConfig = await TtvBackend.invoke('tangxin_cloud_config_get');
  }catch(error){
    console.warn('云端账号池配置读取失败：', error);
  }
  txRenderCloudBar();
}
function txRenderCloudBar(){
  const mode = document.getElementById('txCloudMode');
  const status = document.getElementById('txCloudStatus');
  const config = txState.cloudConfig;
  if(mode && config?.accountSourceMode) mode.value = config.accountSourceMode;
  if(!status) return;
  if(!config){ status.textContent = ''; return; }
  const parts = [];
  if(config.lastSyncAt){
    const synced = new Date(config.lastSyncAt * 1000);
    parts.push('上次同步 ' + synced.toLocaleString('zh-CN', {month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit'}));
  } else {
    parts.push('尚未同步');
  }
  if(config.lastError) parts.push('错误：' + config.lastError);
  status.textContent = parts.join(' · ');
  status.title = config.baseUrl || '';
}
async function txSyncCloud(){
  if(!TtvBackend.available() || txState.cloudBusy) return;
  const button = document.getElementById('txCloudSyncBtn');
  txState.cloudBusy = true;
  if(button){ button.disabled = true; button.textContent = '同步中…'; }
  try{
    const snapshot = await TtvBackend.invoke('tangxin_cloud_sync');
    txState.cloudConfig = snapshot?.config || txState.cloudConfig;
    txState.accounts = snapshot?.accounts || [];
    toast(`云端账号池同步完成：共 ${txState.accounts.length} 个账号`);
  }catch(error){
    toast('云端账号池同步失败：' + backendErrorMessage(error));
    void txLoadCloudConfig();
  }finally{
    txState.cloudBusy = false;
    if(button){ button.disabled = false; button.textContent = '同步云端池'; }
  }
  txRenderAccounts();
}
async function txChangeCloudMode(mode){
  if(!TtvBackend.available()) return;
  try{
    txState.cloudConfig = await TtvBackend.invoke('tangxin_cloud_config_set', {accountSourceMode: mode});
    const labels = {cloud: '云端轮班', 'cloud-first': '云端优先', local: '本地值班'};
    toast(`账号来源已切换：${labels[mode] || mode}。云端池账号需要「同步云端池」拉取。`);
  }catch(error){
    toast('切换账号来源失败：' + backendErrorMessage(error));
  }
  txRenderCloudBar();
}
async function txUploadCloud(id){
  if(!TtvBackend.available() || txState.cloudBusy) return;
  if(!window.confirm('把该账号上传到云端共享池？\n上传后由云端池统一轮换，本地不再单独管理该账号。')) return;
  const button = document.querySelector(`[data-txaccount="${id}"] [data-txact="upload"]`);
  txState.cloudBusy = true;
  if(button){ button.disabled = true; button.textContent = '上传中…'; }
  try{
    const snapshot = await TtvBackend.invoke('tangxin_cloud_upload', {id});
    txState.cloudConfig = snapshot?.config || txState.cloudConfig;
    txState.accounts = snapshot?.accounts || [];
    toast('账号已上传到云端共享池');
  }catch(error){
    toast('上传云端失败：' + backendErrorMessage(error));
  }finally{
    txState.cloudBusy = false;
  }
  txRenderAccounts();
}
async function txLoadAccounts(){
  if(!TtvBackend.available()) return;
  txState.accountsLoading = true;
  try{
    txState.accounts = await TtvBackend.invoke('tangxin_account_list') || [];
  }catch(error){
    toast('糖心账号池加载失败：' + backendErrorMessage(error));
  }finally{
    txState.accountsLoading = false;
  }
  txRenderAccounts();
}
function txRenderAccounts(){
  const list = document.getElementById('txAccountList');
  const count = document.getElementById('txAccountCount');
  if(count){
    const total = txState.accounts.length;
    count.hidden = total === 0;
    count.textContent = String(total);
  }
  if(!list) return;
  if(!txState.accounts.length){
    list.innerHTML = '<p class="tx-account-empty">账号池为空。点击「同步云端池」拉取共享账号，或添加账号密码 / token 凭证；未添加时以访客身份播放（可能仅试看）。</p>';
    return;
  }
  const modeLabels = { password: '账号密码', token: 'token 凭证', qrcode: '二维码凭证' };
  list.innerHTML = txState.accounts.map(account => {
    const chips = [];
    if(account.isCloud) chips.push('<span class="tx-chip tx-chip-cloud">云端</span>');
    if(account.isVip) chips.push('<span class="tx-chip tx-chip-vip">VIP</span>');
    if(account.isDarkVip) chips.push('<span class="tx-chip tx-chip-vip">尤物圈</span>');
    if(account.coin) chips.push(`<span class="tx-chip">金币 ${escapeHtml(account.coin)}</span>`);
    if(account.available) chips.push('<span class="tx-chip tx-chip-ok">可用</span>');
    else chips.push(`<span class="tx-chip tx-chip-bad" title="${escapeHtml(account.unavailableReason || '')}">未验证/失效</span>`);
    if(account.selected) chips.push('<span class="tx-chip tx-chip-sel">优先使用</span>');
    const actions = account.isCloud
      ? '<button type="button" class="btn btn-ghost" data-txact="verify">验证</button>'
      : `<button type="button" class="btn btn-ghost" data-txact="select">${account.selected ? '取消优先' : '优先使用'}</button>
        <button type="button" class="btn btn-ghost" data-txact="verify">验证</button>
        <button type="button" class="btn btn-ghost" data-txact="upload">上传云端</button>
        <button type="button" class="btn btn-ghost" data-txact="remove">删除</button>`;
    return `<div class="tx-account-row" data-txaccount="${escapeHtml(account.id)}">
      <div class="tx-account-main">
        <b>${escapeHtml(account.label || '未命名账号')}</b>
        <span>${escapeHtml(modeLabels[account.credentialMode] || account.credentialMode || '')}${account.credentialHint ? ' · ' + escapeHtml(account.credentialHint) : ''}</span>
      </div>
      <div class="tx-account-chips">${chips.join('')}</div>
      <div class="tx-account-actions">${actions}</div>
    </div>`;
  }).join('');
  list.querySelectorAll('[data-txact]').forEach(button => button.addEventListener('click', async () => {
    const id = button.closest('[data-txaccount]').dataset.txaccount;
    const account = txState.accounts.find(item => item.id === id);
    try{
      if(button.dataset.txact === 'select'){
        await TtvBackend.invoke('tangxin_account_select', {id: account.selected ? '' : id});
      } else if(button.dataset.txact === 'verify'){
        button.disabled = true; button.textContent = '验证中…';
        const updated = await TtvBackend.invoke('tangxin_account_verify', {id});
        toast(updated?.available ? `账号 ${updated.label} 验证通过` : `账号验证失败：${updated?.unavailableReason || '未知原因'}`);
      } else if(button.dataset.txact === 'upload'){
        void txUploadCloud(id);
        return;
      } else if(button.dataset.txact === 'remove'){
        if(!window.confirm(`从账号池删除“${account.label || '该账号'}”？\n只移除本地保存的凭证，不影响站点账号本身。`)) return;
        await TtvBackend.invoke('tangxin_account_remove', {id});
      }
      await txLoadAccounts();
    }catch(error){
      toast('账号操作失败：' + backendErrorMessage(error));
      if(button.dataset.txact === 'verify'){ button.disabled = false; button.textContent = '验证'; }
    }
  }));
}
function txToggleAccounts(force){
  const panel = document.getElementById('txAccountsPanel');
  if(!panel) return;
  const show = typeof force === 'boolean' ? force : panel.hidden;
  panel.hidden = !show;
  if(show){
    void txLoadAccounts();
    void txLoadCloudConfig();
  }
}
function txAccountModeFields(){
  const mode = document.getElementById('txAccountMode')?.value || 'password';
  document.querySelectorAll('#txAccountForm [data-txfield]').forEach(row => {
    row.hidden = row.dataset.txfield !== mode;
  });
}
async function txSaveAccount(){
  const msg = document.getElementById('txAccountFormMsg');
  const mode = document.getElementById('txAccountMode')?.value || 'password';
  const payload = {label: document.getElementById('txAccountLabel')?.value || ''};
  if(mode === 'password'){
    payload.username = document.getElementById('txAccountUsername')?.value || '';
    payload.password = document.getElementById('txAccountPassword')?.value || '';
  } else if(mode === 'token'){
    payload.deviceId = document.getElementById('txAccountDeviceId')?.value || '';
    payload.userToken = document.getElementById('txAccountUserToken')?.value || '';
  } else {
    payload.qrcode = document.getElementById('txAccountQrcode')?.value || '';
  }
  const saveBtn = document.getElementById('txAccountSaveBtn');
  if(saveBtn){ saveBtn.disabled = true; saveBtn.textContent = '正在验证…'; }
  if(msg){ msg.textContent = ''; msg.classList.remove('error'); }
  try{
    const view = await TtvBackend.invoke('tangxin_account_add', payload);
    toast(`账号 ${view?.label || ''} 已加入账号池并验证通过`);
    ['txAccountUsername','txAccountPassword','txAccountDeviceId','txAccountUserToken','txAccountQrcode','txAccountLabel'].forEach(id => {
      const input = document.getElementById(id);
      if(input) input.value = '';
    });
  }catch(error){
    if(msg){ msg.textContent = backendErrorMessage(error); msg.classList.add('error'); }
  }finally{
    if(saveBtn){ saveBtn.disabled = false; saveBtn.textContent = '保存并验证'; }
  }
  await txLoadAccounts();
}

async function playTangxinCurrent(){
  const movie = txState.detailCurrent;
  if(!movie || txState.playRequest) return;
  if(!TtvBackend.available()){ toast('糖心影院需要桌面端连接。'); return; }
  if(!isNativeMediaMode()){ toast('糖心影院播放需要桌面端原生播放器（公开目录模式不支持）。'); return; }
  const request = ++txState.playRequest;
  const allowBuy = Boolean(txState.confirmBuy);
  const button = document.getElementById('txPlayBtn');
  if(button){ button.disabled = true; button.classList.add('loading'); button.textContent = '正在检票…'; }
  toast(allowBuy ? '正在用账号金币解锁并获取完整线路…' : '正在获取播放线路…');
  try{
    const result = await TtvBackend.invoke('tangxin_play', {movieId: String(movie.id), allowBuy});
    if(request !== txState.playRequest) return;
    if(result?.needsPurchase){
      // 金币内容：后端拒绝擅自扣费，需要用户再次点击「开映」确认。
      txState.confirmBuy = {price: result.purchasePrice, accountLabel: result.purchaseAccountLabel};
      const hint = document.getElementById('txPlayHint');
      if(hint) hint.textContent = `该内容需 ${result.purchasePrice} 金币解锁（账号 ${result.purchaseAccountLabel || '轮换'}）。再次点击「开映」即确认扣费解锁。`;
      toast(`金币内容：需 ${result.purchasePrice} 金币解锁，再次点击「开映」确认。`);
      return;
    }
    txState.confirmBuy = null;
    const hint = document.getElementById('txPlayHint');
    if(hint) hint.textContent = '';
    if(!result?.playlist && !result?.playlistPath) throw new Error('播放线路解析失败');
    closeTangxinDetail();
    if(result.isPreview) toast('当前线路为试看片段。在「账号池」添加有权益的糖心账号可观看完整时长。');
    // 糖心 CDN 的 TLS 指纹会拒绝 libmpv/ffmpeg 的握手（CloudFront alert），
    // 浏览器层（Chromium TLS + CDN 全开 CORS）是唯一稳定通路：把 m3u8 文本
    // 包成 blob URL 交给 hls.js，key/分片由 webview 直接请求。
    let blobUrl = '';
    if(result.playlist){
      try{
        blobUrl = URL.createObjectURL(new Blob([result.playlist], {type: 'application/vnd.apple.mpegurl'}));
      }catch(error){ console.warn('Unable to wrap playlist as blob:', error); }
    }
    const pseudo = {
      id: 'tangxin:' + movie.id,
      t: movie.title,
      img: txPosterCache.get(movie.posterUrl) || '/assets/detail-poster.jpg',
      summary: txState.detail?.description || '糖心影院 · 在线放映',
      adult: true,
      playUrl: result.playlistPath || '',
      browserPlayUrl: blobUrl,
      forceWebPlayback: Boolean(blobUrl),
      // 浏览器会拒绝覆写 Referer/UA（forbidden headers），CDN 也不校验，留空最稳。
      playHeaders: blobUrl ? {} : (result.headers || {}),
      sourceLabel: '糖心影院'
        + (result.accountLabel ? ' · ' + result.accountLabel : '')
        + (result.isPreview ? ' · 试看' : ''),
      q: '',
      durationSeconds: Number(movie.durationSeconds) || 0
    };
    await openPlayer(pseudo, null, true);
  }catch(error){
    if(request === txState.playRequest) toast('开映失败：' + backendErrorMessage(error));
  }finally{
    if(request === txState.playRequest && button){ button.disabled = false; button.classList.remove('loading'); button.textContent = '▶ 开映'; txState.playRequest = 0; }
  }
}
function initTangxinCinema(){
  document.querySelectorAll('#view-adult .zone-btn').forEach(btn =>
    btn.addEventListener('click', () => switchAdultZone(btn.dataset.adultZone)));
  const searchInput = document.getElementById('txSearchInput');
  if(searchInput) searchInput.addEventListener('keydown', event => {
    if(event.key === 'Enter') void runTangxinSearch(true);
  });
  const searchBtn = document.getElementById('txSearchBtn');
  if(searchBtn) searchBtn.addEventListener('click', () => void runTangxinSearch(true));
  const discoverBtn = document.getElementById('txDiscoverBtn');
  if(discoverBtn) discoverBtn.addEventListener('click', () => openTangxinZoneFromCinema());
  const moreBtn = document.getElementById('txLoadMoreBtn');
  if(moreBtn) moreBtn.addEventListener('click', () => void runTangxinSearch(false));
  const closeBtn = document.getElementById('txDetailClose');
  if(closeBtn) closeBtn.addEventListener('click', closeTangxinDetail);
  const overlay = document.getElementById('txDetailOverlay');
  if(overlay) overlay.addEventListener('click', event => {
    if(event.target === overlay) closeTangxinDetail();
  });
  const playBtn = document.getElementById('txPlayBtn');
  if(playBtn) playBtn.addEventListener('click', () => void playTangxinCurrent());
  const accountsBtn = document.getElementById('txAccountsBtn');
  if(accountsBtn) accountsBtn.addEventListener('click', () => txToggleAccounts());
  const accountsClose = document.getElementById('txAccountsClose');
  if(accountsClose) accountsClose.addEventListener('click', () => txToggleAccounts(false));
  const accountMode = document.getElementById('txAccountMode');
  if(accountMode) accountMode.addEventListener('change', txAccountModeFields);
  const accountSave = document.getElementById('txAccountSaveBtn');
  if(accountSave) accountSave.addEventListener('click', () => void txSaveAccount());
  const cloudMode = document.getElementById('txCloudMode');
  if(cloudMode) cloudMode.addEventListener('change', () => void txChangeCloudMode(cloudMode.value));
  const cloudSync = document.getElementById('txCloudSyncBtn');
  if(cloudSync) cloudSync.addEventListener('click', () => void txSyncCloud());
}
initTangxinCinema();

/* 路由解析 */
const HASH_VIEWS = ['home', 'library', 'short-drama', 'detail', 'cloud', 'cloud-browser', 'servers', 'downloads', 'profile', 'logs', 'trash', 'settings'];
function routeFromHash(){
  const hv = location.hash.replace('#','');
  if(hv === 'player'){
    openPlayer(currentView === 'home' ? homeMovieAt(current) : selectedMovie);
  } else if(hv === 'scan-progress'){
    history.replaceState(null, '', location.pathname + location.search + '#library');
    showView('library', false);
  } else if(hv === 'my-content'){
    showView('profile', false);
  } else if(hv === 'comic-drama'){
    history.replaceState(null, '', location.pathname + location.search + '#short-drama');
    showView('short-drama', false);
    setHongguoSource('comic');
  } else if(HASH_VIEWS.includes(hv)){
    showView(hv, false);
  } else {
    showView('home', false);
  }
}
window.addEventListener('hashchange', routeFromHash);

/* ========================================================================= */
/* 【极简磨砂玻璃悬浮灵动岛交互控制器】Pure Frosted Glass Island Pro Controller */
/* ========================================================================= */

const PLAYER_STATIC_POPOVER_TRIGGERS = {
  speedMenuPopover: 'chipSpeed',
  qualityMenuPopover: 'chipQuality',
  epPickerPopover: 'btnEpisode'
};
let playerActionPopoverAnchor = null;
let playerActionPopoverKind = '';
let playerActionPopoverToken = 0;
let audioPanelAnchor = null;

function resolvePlayerPopoverAnchor(source, fallbackId){
  const candidate = source?.currentTarget || source?.target || source;
  if(candidate instanceof Element) return candidate.closest('button') || candidate;
  return fallbackId ? document.getElementById(fallbackId) : null;
}

function setStaticPlayerPopoverState(id, open){
  const popover = document.getElementById(id);
  const trigger = document.getElementById(PLAYER_STATIC_POPOVER_TRIGGERS[id]);
  if(popover) popover.setAttribute('aria-hidden', open ? 'false' : 'true');
  if(trigger) trigger.setAttribute('aria-expanded', open ? 'true' : 'false');
}

function closePlayerActionPopover(restoreFocus = false){
  const popover = document.getElementById('playerActionPopover');
  const anchor = playerActionPopoverAnchor;
  const wasOpen = !!popover?.classList.contains('show');
  if(popover){
    popover.classList.remove('show', 'opens-downward', 'is-compact', 'is-wide');
    popover.setAttribute('aria-hidden', 'true');
    popover.removeAttribute('data-placement');
  }
  if(anchor){
    anchor.setAttribute('aria-expanded', 'false');
    if(restoreFocus && wasOpen && anchor.isConnected) anchor.focus({preventScroll:true});
  }
  playerActionPopoverAnchor = null;
  playerActionPopoverKind = '';
  playerActionPopoverToken += 1;
  return wasOpen;
}

function closeAudioPanel(restoreFocus = false){
  const panel = document.getElementById('audioPanel');
  const anchor = audioPanelAnchor || document.getElementById('btnAudioEffects');
  const wasOpen = !!panel && !panel.classList.contains('tvv-audio-hidden');
  panel?.classList.add('tvv-audio-hidden');
  panel?.setAttribute('aria-hidden', 'true');
  anchor?.setAttribute('aria-expanded', 'false');
  if(restoreFocus && wasOpen && anchor?.isConnected) anchor.focus({preventScroll:true});
  audioPanelAnchor = null;
  return wasOpen;
}

function closeAllIslandPopovers(exceptId = null, restoreFocus = false){
  ['speedMenuPopover', 'qualityMenuPopover', 'epPickerPopover'].forEach(id => {
    if(id === exceptId) return;
    const el = document.getElementById(id);
    if(el) el.classList.remove('show');
    setStaticPlayerPopoverState(id, false);
  });
  if(exceptId !== 'playerActionPopover') closePlayerActionPopover(restoreFocus);
  if(exceptId !== 'audioPanel') closeAudioPanel(restoreFocus);
}

function hasOpenPlayerPopover(){
  return !!document.querySelector('#view-player .popover-menu.show, #view-player .ep-popover-menu.show, #view-player .more-popover-menu.show, #playerActionPopover.show')
    || !!document.getElementById('audioPanel') && !document.getElementById('audioPanel').classList.contains('tvv-audio-hidden');
}

function positionPlayerActionPopover(){
  const popover = document.getElementById('playerActionPopover');
  const anchor = playerActionPopoverAnchor;
  if(!popover?.classList.contains('show') || !anchor?.isConnected) return;
  const rect = anchor.getBoundingClientRect();
  const viewportWidth = document.documentElement.clientWidth || window.innerWidth;
  const viewportHeight = document.documentElement.clientHeight || window.innerHeight;
  const margin = 12;
  const gap = 10;
  const width = popover.offsetWidth;
  const height = popover.offsetHeight;
  const availableAbove = rect.top - margin - gap;
  const availableBelow = viewportHeight - rect.bottom - margin - gap;
  const opensDownward = availableAbove < Math.min(height, 220) && availableBelow > availableAbove;
  let left = rect.left + rect.width / 2 - width / 2;
  left = Math.min(Math.max(margin, left), Math.max(margin, viewportWidth - width - margin));
  let top = opensDownward ? rect.bottom + gap : rect.top - gap - height;
  top = Math.min(Math.max(margin, top), Math.max(margin, viewportHeight - height - margin));
  const arrowX = Math.min(Math.max(18, rect.left + rect.width / 2 - left), Math.max(18, width - 18));
  popover.style.left = Math.round(left) + 'px';
  popover.style.top = Math.round(top) + 'px';
  popover.style.setProperty('--player-popover-arrow-x', Math.round(arrowX) + 'px');
  popover.classList.toggle('opens-downward', opensDownward);
  popover.dataset.placement = opensDownward ? 'bottom' : 'top';
}

function positionAudioPanel(){
  const panel = document.getElementById('audioPanel');
  const anchor = audioPanelAnchor;
  if(!panel || panel.classList.contains('tvv-audio-hidden') || !anchor?.isConnected) return;
  const rect = anchor.getBoundingClientRect();
  const viewportWidth = document.documentElement.clientWidth || window.innerWidth;
  const viewportHeight = document.documentElement.clientHeight || window.innerHeight;
  const margin = 12;
  const gap = 10;
  const width = panel.offsetWidth;
  const height = panel.offsetHeight;
  let left = rect.left + rect.width / 2 - width / 2;
  left = Math.min(Math.max(margin, left), Math.max(margin, viewportWidth - width - margin));
  let top = rect.top - gap - height;
  if(top < margin) top = Math.min(viewportHeight - height - margin, rect.bottom + gap);
  panel.style.left = Math.round(left) + 'px';
  panel.style.top = Math.max(margin, Math.round(top)) + 'px';
  panel.style.right = 'auto';
  panel.style.bottom = 'auto';
}

function refreshPlayerActionPopoverLayout(){
  requestAnimationFrame(() => requestAnimationFrame(positionPlayerActionPopover));
}

function openPlayerActionPopover({kind, title, hint = '', bodyHtml = '', anchor, variant = ''}){
  const popover = document.getElementById('playerActionPopover');
  const titleEl = document.getElementById('playerActionPopoverTitle');
  const hintEl = document.getElementById('playerActionPopoverHint');
  const bodyEl = document.getElementById('playerActionPopoverBody');
  if(!popover || !titleEl || !hintEl || !bodyEl || !anchor) return null;
  if(popover.classList.contains('show') && playerActionPopoverKind === kind && playerActionPopoverAnchor === anchor){
    closePlayerActionPopover(true);
    uiActivity();
    return null;
  }
  closeAllIslandPopovers('playerActionPopover');
  closePlayerActionPopover(false);
  playerActionPopoverAnchor = anchor;
  playerActionPopoverKind = kind;
  const token = ++playerActionPopoverToken;
  titleEl.textContent = title;
  hintEl.textContent = hint;
  hintEl.hidden = !hint;
  bodyEl.innerHTML = bodyHtml;
  popover.classList.toggle('is-compact', variant === 'compact');
  popover.classList.toggle('is-wide', variant === 'wide');
  popover.classList.add('show');
  popover.setAttribute('aria-hidden', 'false');
  anchor.setAttribute('aria-expanded', 'true');
  anchor.setAttribute('aria-controls', 'playerActionPopover');
  refreshPlayerActionPopoverLayout();
  uiActivity();
  return token;
}

function updatePlayerActionPopover(token, bodyHtml){
  const popover = document.getElementById('playerActionPopover');
  const bodyEl = document.getElementById('playerActionPopoverBody');
  if(!popover?.classList.contains('show') || token !== playerActionPopoverToken || !bodyEl) return false;
  bodyEl.innerHTML = bodyHtml;
  refreshPlayerActionPopoverLayout();
  return true;
}

function closeOpenPlayerPopover(){
  const dynamicClosed = closePlayerActionPopover(true);
  let staticClosed = false;
  ['speedMenuPopover', 'qualityMenuPopover', 'epPickerPopover'].forEach(id => {
    const popover = document.getElementById(id);
    if(popover?.classList.contains('show')){
      staticClosed = true;
      popover.classList.remove('show');
      setStaticPlayerPopoverState(id, false);
      document.getElementById(PLAYER_STATIC_POPOVER_TRIGGERS[id])?.focus({preventScroll:true});
    }
  });
  const audioClosed = closeAudioPanel(true);
  return dynamicClosed || staticClosed || audioClosed;
}

// 点击空白关闭浮动面板；面板内部与当前触发按钮上的操作保持打开。
document.addEventListener('click', (e) => {
  if(!e.target.closest('.dropdown-anchor')){
    ['speedMenuPopover', 'qualityMenuPopover', 'epPickerPopover'].forEach(id => {
      document.getElementById(id)?.classList.remove('show');
      setStaticPlayerPopoverState(id, false);
    });
  }
  const insideDynamic = !!e.target.closest('#playerActionPopover');
  const insideAnchor = !!playerActionPopoverAnchor?.contains(e.target);
  if(!insideDynamic && !insideAnchor) closePlayerActionPopover(false);
  const insideAudioPanel = !!e.target.closest('#audioPanel');
  const insideAudioAnchor = !!audioPanelAnchor?.contains(e.target);
  if(!insideAudioPanel && !insideAudioAnchor) closeAudioPanel(false);
});
window.addEventListener('resize', () => {
  refreshPlayerActionPopoverLayout();
  requestAnimationFrame(positionAudioPanel);
});
document.addEventListener('fullscreenchange', () => {
  refreshPlayerActionPopoverLayout();
  requestAnimationFrame(positionAudioPanel);
});

/* 1. 垂直倍速菜单展开与选择 */
function toggleSpeedMenu(e){
  e?.stopPropagation();
  const pop = document.getElementById('speedMenuPopover');
  const isShown = pop?.classList.contains('show');
  closeAllIslandPopovers();
  if(!isShown && pop){
    pop.classList.add('show');
    setStaticPlayerPopoverState('speedMenuPopover', true);
  }
  uiActivity();
}

function selectSpeedDirect(spd){
  applyPlaybackSpeed(Number.parseFloat(spd) || 1.0);
  document.querySelectorAll('#speedMenuPopover .popover-opt').forEach(opt => {
    opt.classList.toggle('active', opt.textContent.trim() === spd);
  });
  closeAllIslandPopovers();
}

/* 2. 垂直画质菜单展开与选择 */
function toggleQualityMenu(e){
  e?.stopPropagation();
  const pop = document.getElementById('qualityMenuPopover');
  const isShown = pop?.classList.contains('show');
  closeAllIslandPopovers();
  if(!isShown && pop){
    // 根据当前视频的 versions 动态组装或采用常见清晰度
    renderQualityMenuOptions();
    pop.classList.add('show');
    setStaticPlayerPopoverState('qualityMenuPopover', true);
  }
  uiActivity();
}

function renderQualityMenuOptions(){
  const pop = document.getElementById('qualityMenuPopover');
  if(!pop) return;
  const entries = qualityEntriesFor(selectedMovie);
  const curQ = (document.getElementById('chipQuality')?.textContent || '').trim();
  if(entries.length > 0){
    pop.innerHTML = entries.map((entry, i) => {
      const label = entry.label;
      const isActive = curQ === label || (curQ === '自动' && entry.isDefault);
      const badge = entry.needVip ? '<small class="quality-vip-badge">会员</small>' : '';
      return `<div class="popover-opt${isActive ? ' active' : ''}" onclick="selectQualityFromVersion(${i})"><span class="quality-option-label">${escapeHtml(label)}</span>${badge}</div>`;
    }).join('');
  }else{
    pop.innerHTML = '<div class="popover-opt quality-option-empty">当前视频没有可切换的真实画质</div>';
  }
}

async function selectQualityFromVersion(index, label){
  const entries = qualityEntriesFor(selectedMovie);
  const entry = entries[index];
  if(!entry) return;
  closeAllIslandPopovers();
  toast('正在切换画质: ' + (label || entry.label));
  await switchToQuality({...entry, label: label || entry.label});
}

function selectQualityDirect(q){
  closeAllIslandPopovers();
  const entries = qualityEntriesFor(selectedMovie);
  const match = entries.find(entry => entry.label === q || entry.label.includes(q) || q.includes(entry.label));
  if(match){
    void selectQualityFromVersion(entries.indexOf(match), match.label);
    return;
  }
  toast('当前视频没有可切换的真实画质流');
}

/* 3. 选集垂直向上展开与集数切换 */
function toggleEpisodePicker(e){
  e?.stopPropagation();
  const pop = document.getElementById('epPickerPopover');
  const isShown = pop?.classList.contains('show');
  closeAllIslandPopovers();
  if(!isShown && pop){
    renderEpisodePickerGrid();
    pop.classList.add('show');
    setStaticPlayerPopoverState('epPickerPopover', true);
  }
  uiActivity();
}

function renderEpisodePickerGrid(){
  const grid = document.getElementById('epPickerGrid');
  const labelEl = document.getElementById('epPickerActiveLabel');
  if(!grid) return;
  const parent = selectedMovie?.episodes?.length ? selectedMovie : detailMovie;
  const episodes = Array.isArray(parent?.episodes) ? parent.episodes : [];
  if(!episodes.length){
    grid.innerHTML = '<div style="grid-column:1/-1;font-size:12px;color:var(--text-faint);text-align:center;padding:12px 0">当前内容没有可选集数</div>';
    if(labelEl) labelEl.textContent = '单本正片';
    return;
  }
  const activeIndex = activeEpisodeIndex(episodes);
  if(labelEl){
    labelEl.textContent = activeIndex >= 0 ? `当前第 ${activeIndex + 1} 集` : '请选择剧集';
  }
  grid.innerHTML = episodes.map((ep, idx) => {
    const isCur = idx === activeIndex;
    const numStr = String(idx + 1).padStart(2, '0');
    return `<button class="ep-item-btn${isCur ? ' active' : ''}" onclick="selectEpisodeDirect(${idx})">${numStr}</button>`;
  }).join('');
}

function selectEpisodeDirect(index){
  closeAllIslandPopovers();
  const parent = selectedMovie?.episodes?.length ? selectedMovie : detailMovie;
  playEpisode(index, null, parent);
}

/* 4. 岛栏功能直控（旧版"更多面板"相关死代码已于 2026-09-02 清理，chip 直控保留） */

function toggleDanmakuFromIsland(){
  const chipDanmaku = document.getElementById('chipDanmaku');
  const fakeBtn = chipDanmaku || document.createElement('button');
  toggleDanmaku(fakeBtn);
}

/* 初始执行 */
renderProgress();
normalizeCloudProviderIcons();
setupScrapeProgressListener();
setupScanProgressListener();
setupSourceDeleteProgressListener();
restoreScanTasks();
renderNotificationTasks();
loadSourceCatalog()
  .then(() => autoReclassifyAdultOnce())
  .then(() => loadInitialCatalog())
  .then(refreshProviderStatus)
  .finally(() => window.setTimeout(resumePersistedScrapeTasks, 5000));
renderWatchlist();
// A WebView2 renderer restart can reload this page while the Rust process and
// its libmpv actor survive. Always clear that actor before restoring the route;
// waiting for the close avoids a race where #player immediately creates a new
// actor and the stale cleanup then tears down the new one.
async function initializeRouteAfterPlaybackCleanup(){
  if(TtvBackend.available()){
    await TtvBackend.invoke('player_native_close').catch(error => {
      console.warn('Unable to clear stale native playback on startup:', error);
    });
  }
  routeFromHash();
  if(!location.hash) stagger(document.getElementById('view-home'));
}
void initializeRouteAfterPlaybackCleanup();
