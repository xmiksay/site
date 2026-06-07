// Markdown editor toolbar
(function () {
    const toolbar = document.getElementById('md-toolbar');
    const textarea = document.getElementById('markdown') || document.getElementById('body_md');
    if (!toolbar || !textarea) return;

    const buttons = [
        { label: 'B', prefix: '**', suffix: '**' },
        { label: 'I', prefix: '_', suffix: '_' },
        { label: 'H2', prefix: '## ', suffix: '' },
        { label: 'H3', prefix: '### ', suffix: '' },
        { label: 'Link', prefix: '[', suffix: '](url)' },
        { label: 'Img', prefix: '[img ', suffix: ']' },
        { label: 'Code', prefix: '`', suffix: '`' },
    ];

    buttons.forEach(b => {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.textContent = b.label;
        btn.addEventListener('click', () => {
            const start = textarea.selectionStart;
            const end = textarea.selectionEnd;
            const text = textarea.value;
            const selected = text.substring(start, end);
            textarea.value = text.substring(0, start) + b.prefix + selected + b.suffix + text.substring(end);
            textarea.focus();
            textarea.selectionStart = start + b.prefix.length;
            textarea.selectionEnd = start + b.prefix.length + selected.length;
        });
        toolbar.appendChild(btn);
    });
})();
