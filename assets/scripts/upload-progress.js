// Shared upload helper (Phase CK): POST via XMLHttpRequest with REAL upload
// progress. `fetch()` exposes no upload progress (no bytes-sent events); XHR's
// `upload.onprogress` does. Used by the media library (media-upload.js) and the
// inline editor (editor-support.js) — load this BEFORE either.
(function () {
  function fmtBytes(n) {
    if (!Number.isFinite(n) || n < 0) return "";
    if (n < 1024) return n + " B";
    const units = ["KB", "MB", "GB", "TB"];
    let i = -1;
    do {
      n /= 1024;
      i += 1;
    } while (n >= 1024 && i < units.length - 1);
    return n.toFixed(1) + " " + units[i];
  }

  // POST `formData` to `url` via XHR. `onProgress(phase, loaded, total)` fires
  // with phase "uploading" (loaded/total bytes sent) and then "processing" once
  // the body is fully sent and the server is ingesting. Resolves with the
  // response text (caller parses), rejects with an error string.
  function xhrUpload(url, formData, onProgress) {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open("POST", url);
      xhr.upload.addEventListener("progress", (e) => {
        if (e.lengthComputable) onProgress("uploading", e.loaded, e.total);
      });
      // Body fully sent → the server is now probing/renaming the stored file.
      xhr.upload.addEventListener("load", () => onProgress("processing", 0, 0));
      xhr.addEventListener("load", () => {
        if (xhr.status >= 200 && xhr.status < 300) resolve(xhr.responseText);
        else reject(xhr.responseText || "HTTP " + xhr.status);
      });
      xhr.addEventListener("error", () => reject("network error"));
      xhr.addEventListener("abort", () => reject("aborted"));
      xhr.send(formData);
    });
  }

  window.UploadProgress = { fmtBytes, xhrUpload };
})();
