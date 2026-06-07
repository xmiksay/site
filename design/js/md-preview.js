// Markdown live preview
const textarea = document.getElementById('markdown') || document.getElementById('body_md');
const preview = document.getElementById('md-preview');
const toggle = document.getElementById('toggle-preview');

if (textarea && preview && toggle) {
    let showing = false;

    toggle.addEventListener('click', () => {
        showing = !showing;
        if (showing) {
            preview.style.display = 'block';
            preview.innerHTML = typeof marked !== 'undefined' ? marked.parse(textarea.value) : textarea.value;
        } else {
            preview.style.display = 'none';
        }
    });

    textarea.addEventListener('input', () => {
        if (showing && typeof marked !== 'undefined') {
            preview.innerHTML = marked.parse(textarea.value);
        }
    });
}
