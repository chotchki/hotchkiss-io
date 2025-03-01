//From https://stackoverflow.com/a/34278578/160208
function addLink() {
    const el = document.getElementById("page_markdown");
    const [start, end] = [el.selectionStart, el.selectionEnd];
    const currentText = el.value.slice(start, end);
    el.setRangeText("[" + currentText + "]()", start, end, 'select');
    el.dispatchEvent(new Event('change', { bubbles: true }));
}

function addImage() {
    const el = document.getElementById("page_markdown");
    const [start, end] = [el.selectionStart, el.selectionEnd];
    const currentText = el.value.slice(start, end);
    el.setRangeText("![" + currentText + "]()", start, end, 'select');
    el.dispatchEvent(new Event('change', { bubbles: true }));
}

function addAttachment(event) {
    event.preventDefault();
    const el = document.getElementById("page_markdown");
    const targetUrl = event.currentTarget.href;

    const [start, end] = [el.selectionStart, el.selectionEnd];
    const currentText = el.value.slice(start, end);
    el.setRangeText("![" + currentText + "](" + targetUrl + ")", start, end, 'select');
    el.dispatchEvent(new Event('change', { bubbles: true }));
}