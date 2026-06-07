(function () {
    'use strict';

    function copyText(text) {
        if (navigator.clipboard && navigator.clipboard.writeText) {
            return navigator.clipboard.writeText(text);
        }
        // Fallback for older browsers / non-secure contexts.
        return new Promise(function (resolve, reject) {
            try {
                var ta = document.createElement('textarea');
                ta.value = text;
                ta.setAttribute('readonly', '');
                ta.style.position = 'absolute';
                ta.style.left = '-9999px';
                document.body.appendChild(ta);
                ta.select();
                document.execCommand('copy');
                document.body.removeChild(ta);
                resolve();
            } catch (err) {
                reject(err);
            }
        });
    }

    function enhanceCodeBlocks(root) {
        root = root || document;
        root.querySelectorAll('pre.code-block:not([data-codebox-ready])').forEach(function (pre) {
            pre.dataset.codeboxReady = '1';

            var lang = pre.getAttribute('data-lang') || '';

            var box = document.createElement('div');
            box.className = 'code-box';

            var header = document.createElement('div');
            header.className = 'code-box-header';

            var langSpan = document.createElement('span');
            langSpan.className = 'code-box-lang';
            langSpan.textContent = lang;

            var copyBtn = document.createElement('button');
            copyBtn.type = 'button';
            copyBtn.className = 'code-box-copy';
            copyBtn.textContent = 'Copy';

            header.appendChild(langSpan);
            header.appendChild(copyBtn);

            // Insert the wrapper before the pre, then move the pre inside it.
            pre.parentNode.insertBefore(box, pre);
            box.appendChild(header);
            box.appendChild(pre);

            copyBtn.addEventListener('click', function () {
                copyText(pre.innerText).then(function () {
                    copyBtn.textContent = 'Copied';
                    setTimeout(function () { copyBtn.textContent = 'Copy'; }, 1200);
                }).catch(function () {
                    copyBtn.textContent = 'Copy';
                });
            });
        });
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', function () { enhanceCodeBlocks(document); });
    } else {
        enhanceCodeBlocks(document);
    }

    document.addEventListener('content:updated', function (e) {
        enhanceCodeBlocks(e.detail && e.detail.root ? e.detail.root : document);
    });
})();
