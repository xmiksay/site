(function () {
    'use strict';

    let overlay, imgEl, caption, counter, btnPrev, btnNext, btnClose;
    let items = [];
    let current = 0;
    let initialized = false;

    function create() {
        overlay = document.createElement('div');
        overlay.className = 'lightbox-overlay';
        overlay.innerHTML =
            '<button class="lb-close" aria-label="Zavřít">&times;</button>' +
            '<button class="lb-prev" aria-label="Předchozí">&#8249;</button>' +
            '<button class="lb-next" aria-label="Následující">&#8250;</button>' +
            '<div class="lb-content">' +
                '<img class="lb-img" alt="">' +
                '<div class="lb-bottom">' +
                    '<span class="lb-caption"></span>' +
                    '<span class="lb-counter"></span>' +
                '</div>' +
            '</div>';
        document.body.appendChild(overlay);

        imgEl   = overlay.querySelector('.lb-img');
        caption = overlay.querySelector('.lb-caption');
        counter = overlay.querySelector('.lb-counter');
        btnPrev = overlay.querySelector('.lb-prev');
        btnNext = overlay.querySelector('.lb-next');
        btnClose = overlay.querySelector('.lb-close');

        btnClose.addEventListener('click', close);
        btnPrev.addEventListener('click', function () { go(current - 1); });
        btnNext.addEventListener('click', function () { go(current + 1); });
        overlay.addEventListener('click', function (e) {
            if (e.target === overlay || e.target.classList.contains('lb-content')) close();
        });
    }

    function open(group, startIndex) {
        items = group;
        current = startIndex;
        overlay.classList.add('active');
        document.body.style.overflow = 'hidden';
        show();
    }

    function close() {
        overlay.classList.remove('active');
        document.body.style.overflow = '';
        imgEl.src = '';
    }

    function go(index) {
        if (index < 0) index = items.length - 1;
        if (index >= items.length) index = 0;
        current = index;
        show();
    }

    function show() {
        var item = items[current];
        imgEl.src = item.src;
        caption.textContent = item.caption || '';
        if (items.length > 1) {
            counter.textContent = (current + 1) + ' / ' + items.length;
            btnPrev.style.display = '';
            btnNext.style.display = '';
        } else {
            counter.textContent = '';
            btnPrev.style.display = 'none';
            btnNext.style.display = 'none';
        }
    }

    function handleKey(e) {
        if (!overlay.classList.contains('active')) return;
        if (e.key === 'Escape') close();
        else if (e.key === 'ArrowLeft') go(current - 1);
        else if (e.key === 'ArrowRight') go(current + 1);
    }

    function init() {
        // Idempotent: overlay + global listeners are created exactly once.
        if (initialized) return;
        initialized = true;

        create();
        document.addEventListener('keydown', handleKey);

        document.addEventListener('click', function (e) {
            var link = e.target.closest('[data-lightbox]');
            if (!link) return;
            e.preventDefault();

            var groupName = link.getAttribute('data-lightbox');
            var groupLinks = document.querySelectorAll('[data-lightbox="' + groupName + '"]');
            var group = [];
            var startIndex = 0;

            groupLinks.forEach(function (el, i) {
                group.push({
                    src: el.href,
                    caption: el.getAttribute('data-caption') || ''
                });
                if (el === link) startIndex = i;
            });

            open(group, startIndex);
        });
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }

    // Contract hook: click delegation on `document` already covers
    // dynamically-added content, so this simply ensures init() has run.
    // It does not bind any per-call global listeners (init() is idempotent).
    document.addEventListener('content:updated', function () { init(); });
})();
