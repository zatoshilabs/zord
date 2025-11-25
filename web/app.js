const targetSelectors = {
    inscriptions: 'inscription-feed',
    tokens: 'token-table',
    names: 'name-table',
};

const numberFormatter = new Intl.NumberFormat('en-US');

const kindFromContentType = (type = '') => {
    const lower = type.toLowerCase();
    if (lower.includes('json')) return 'json';
    if (lower.startsWith('text/')) return 'text';
    if (lower.startsWith('image/')) return 'image';
    return 'binary';
};

const formatNumber = (value) => {
    if (value === null || value === undefined || Number.isNaN(value)) {
        return '—';
    }
    return numberFormatter.format(value);
};

const formatBytes = (bytes) => {
    if (!bytes && bytes !== 0) return '—';
    const units = ['bytes', 'KB', 'MB', 'GB'];
    let size = bytes;
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
        size /= 1024;
        unit += 1;
    }
    const precise = unit === 0 ? Math.round(size) : Number(size.toFixed(size < 10 ? 1 : 0));
    return `${precise} ${units[unit]}`;
};

const truncateAddress = (value = '', head = 6, tail = 4) => {
    if (!value) return 'unknown';
    if (value.length <= head + tail + 3) return value;
    return `${value.slice(0, head)}…${value.slice(-tail)}`;
};

class PaginatedComponent extends HTMLElement {
    connectedCallback() {
        this.page = 0;
        this.limit = parseInt(this.getAttribute('page-size') || '24', 10);
        this.query = this.getAttribute('search-query') || '';
        this.hasMore = true;
        this.setup();
        this.fetchPage();
    }

    static get observedAttributes() {
        return ['search-query'];
    }

    attributeChangedCallback(name, oldValue, newValue) {
        if (name === 'search-query' && oldValue !== newValue) {
            this.query = newValue;
            this.page = 0;
            this.fetchPage();
        }
    }

    setup() {
        this.container = document.createElement('div');
        this.appendChild(this.container);
    }

    setPlaceholder(message, className = 'loading') {
        this.container.innerHTML = '';
        const div = document.createElement('div');
        div.className = className;
        div.textContent = message;
        this.container.appendChild(div);
    }

    async fetchPage() {
        this.setPlaceholder('Loading…');
        try {
            const q = this.query ? `&q=${encodeURIComponent(this.query)}` : '';
            const res = await fetch(`${this.endpoint}?page=${this.page}&limit=${this.limit}${q}`);
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            const data = await res.json();
            this.hasMore = data.has_more;
            this.render(data.items);
        } catch (err) {
            console.error(err);
            this.setPlaceholder('Unable to load data', 'empty');
        }
    }

    go(direction) {
        if (direction < 0 && this.page === 0) return;
        if (direction > 0 && !this.hasMore) return;
        this.page = Math.max(0, this.page + direction);
        this.fetchPage();
    }
}

class InscriptionFeed extends PaginatedComponent {
    setup() {
        super.setup();
        this.grid = document.createElement('div');
        this.grid.className = 'grid';
        this.typeFilter = 'all';
        this.items = [];
        this.container.appendChild(this.grid);
    }

    get endpoint() {
        return '/api/v1/inscriptions';
    }

    render(items) {
        if (!items.length) {
            this.setPlaceholder('No inscriptions yet', 'empty');
            return;
        }

        this.items = Array.isArray(items) ? items : [];
        this.renderCards();
    }

    setTypeFilter(kind) {
        this.typeFilter = kind || 'all';
        this.renderCards();
    }

    renderCards() {
        this.container.innerHTML = '';
        this.container.appendChild(this.grid);
        this.grid.innerHTML = '';

        const filtered = this.items.filter((item) => {
            const kind = kindFromContentType(item.content_type);
            return this.typeFilter === 'all' || kind === this.typeFilter;
        });

        if (!filtered.length) {
            this.setPlaceholder('No inscriptions for selection', 'empty');
            return;
        }

        filtered.forEach((item) => {
            const kind = kindFromContentType(item.content_type);
            const card = document.createElement('article');
            card.className = 'card';
            card.dataset.kind = kind;

            const header = document.createElement('header');
            const idLink = document.createElement('a');
            idLink.href = `/inscription/${item.id}`;
            idLink.textContent = item.id.slice(0, 12) + '…';
            header.appendChild(idLink);

            const typeTag = document.createElement('span');
            typeTag.textContent = kind;
            header.appendChild(typeTag);
            card.appendChild(header);

            if (kind === 'json') {
                const pre = document.createElement('pre');
                pre.textContent = 'loading json…';
                card.appendChild(pre);
                fetch(`/content/${item.id}`)
                    .then((resp) => resp.text())
                    .then((text) => {
                        try {
                            const pretty = JSON.stringify(JSON.parse(text), null, 2);
                            pre.textContent = pretty.slice(0, 1200);
                        } catch (err) {
                            pre.textContent = text.slice(0, 1200);
                        }
                    })
                    .catch(() => {
                        pre.textContent = 'unable to load preview';
                    });
            } else {
                const code = document.createElement('code');
                if (kind === 'text' && item.preview_text) {
                    code.textContent = item.preview_text;
                } else {
                    code.textContent = `${item.content_type} · ${formatBytes(item.content_length)}`;
                }
                card.appendChild(code);
            }

            const footer = document.createElement('footer');
            const addr = document.createElement('span');
            addr.textContent = truncateAddress(item.sender);
            footer.appendChild(addr);
            if (item.block_height) {
                const height = document.createElement('span');
                height.textContent = `h${formatNumber(item.block_height)}`;
                footer.appendChild(height);
            }
            card.appendChild(footer);

            this.grid.appendChild(card);
        });
    }
}

class TokenTable extends PaginatedComponent {
    setup() {
        super.setup();
        this.table = document.createElement('table');
        const thead = document.createElement('thead');
        thead.innerHTML = '<tr><th>Ticker</th><th>Minted</th><th>Max</th><th>Limit</th><th>Progress</th><th>Inscription</th></tr>';
        this.table.appendChild(thead);
        this.tbody = document.createElement('tbody');
        this.table.appendChild(this.tbody);
        this.container.appendChild(this.table);
    }

    get endpoint() {
        return '/api/v1/tokens';
    }

    render(items) {
        this.tbody.innerHTML = '';
        if (!items.length) {
            this.setPlaceholder('No tokens deployed', 'empty');
            return;
        }

        this.container.innerHTML = '';
        this.container.appendChild(this.table);
        items.forEach((token) => {
            const row = document.createElement('tr');
            const ticker = document.createElement('td');
            ticker.textContent = token.ticker.toUpperCase();
            row.appendChild(ticker);

            const supply = document.createElement('td');
            supply.textContent = token.supply;
            row.appendChild(supply);

            const max = document.createElement('td');
            max.textContent = token.max;
            row.appendChild(max);

            const lim = document.createElement('td');
            lim.textContent = token.lim;
            row.appendChild(lim);

            const progressCell = document.createElement('td');
            const bar = document.createElement('div');
            bar.className = 'progress';
            const fill = document.createElement('span');
            fill.style.width = `${(token.progress * 100).toFixed(2)}%`;
            bar.appendChild(fill);
            progressCell.appendChild(bar);
            row.appendChild(progressCell);

            const insc = document.createElement('td');
            const link = document.createElement('a');
            link.href = `/inscription/${token.inscription_id}`;
            link.textContent = `${token.inscription_id.slice(0, 8)}…`;
            insc.appendChild(link);
            row.appendChild(insc);

            this.tbody.appendChild(row);
        });
    }
}

class NameTable extends PaginatedComponent {
    setup() {
        super.setup();
        this.list = document.createElement('ul');
        this.container.appendChild(this.list);
    }

    get endpoint() {
        return '/api/v1/names';
    }

    render(items) {
        if (!items.length) {
            this.setPlaceholder('No names registered', 'empty');
            return;
        }

        this.container.innerHTML = '';
        this.container.appendChild(this.list);
        this.list.innerHTML = '';
        items.forEach((entry) => {
            const li = document.createElement('li');
            const name = document.createElement('strong');
            name.textContent = entry.name;
            li.appendChild(name);

            const owner = document.createElement('span');
            owner.textContent = truncateAddress(entry.owner);
            li.appendChild(owner);

            const link = document.createElement('a');
            link.href = `/inscription/${entry.inscription_id}`;
            link.textContent = entry.inscription_id.slice(0, 8) + '…';
            li.appendChild(link);

            this.list.appendChild(li);
        });
    }
}

class ZordStatus extends HTMLElement {
    connectedCallback() {
        this.renderSkeleton();
        this.refresh();
        this.timer = setInterval(() => this.refresh(), 15000);
    }

    disconnectedCallback() {
        clearInterval(this.timer);
    }

    renderSkeleton() {
        this.innerHTML = '<p class="loading">Loading status…</p>';
    }

    async refresh() {
        try {
            const res = await fetch('/api/v1/status');
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            const data = await res.json();
            this.innerHTML = '';
            const height = document.createElement('div');
            height.innerHTML = `<strong>Height</strong><br><status-value>${data.height ?? '—'}</status-value>`;
            this.appendChild(height);

            const grid = document.createElement('div');
            grid.className = 'status-grid';
            grid.innerHTML = `
                <div><div>Inscriptions</div><status-value>${data.inscriptions}</status-value></div>
                <div><div>Tokens</div><status-value>${data.tokens}</status-value></div>
                <div><div>Names</div><status-value>${data.names}</status-value></div>
                <div><div>Version</div><status-value>${data.version}</status-value></div>
            `;
            this.appendChild(grid);
        } catch (err) {
            console.error(err);
            this.innerHTML = '<p class="empty">Status offline</p>';
        }
    }
}

customElements.define('inscription-feed', InscriptionFeed);
customElements.define('token-table', TokenTable);
customElements.define('name-table', NameTable);
customElements.define('zord-status', ZordStatus);

(function registerActions() {
    document.addEventListener('click', (event) => {
        const target = event.target.closest('button[data-target][data-action]');
        if (!target) return;
        const selector = targetSelectors[target.dataset.target];
        if (!selector) return;
        const element = document.querySelector(selector);
        if (!element || typeof element.go !== 'function') return;
        const delta = target.dataset.action === 'next' ? 1 : -1;
        element.go(delta);
    });

    const typeFilter = document.getElementById('type-filter');
    if (typeFilter) {
        typeFilter.addEventListener('change', (event) => {
            const feed = document.querySelector('inscription-feed');
            if (feed && typeof feed.setTypeFilter === 'function') {
                feed.setTypeFilter(event.target.value);
            }
        });
    }
})();
