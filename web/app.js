const targetSelectors = {
    inscriptions: 'inscription-feed',
    tokens: 'token-table',
    names: 'name-table',
};

const numberFormatter = new Intl.NumberFormat('en-US');

const kindFromContentType = (type = '') => {
    const lower = type.toLowerCase();
    if (lower === 'image/png') return 'png';
    if (lower === 'image/jpeg' || lower === 'image/jpg') return 'jpeg';
    if (lower === 'image/gif') return 'gif';
    if (lower === 'image/svg+xml') return 'svg';
    if (lower === 'text/html' || lower === 'application/xhtml+xml') return 'html';
    if (lower === 'text/javascript' || lower === 'application/javascript') return 'javascript';
    if (lower.startsWith('text/')) return 'text';
    if (lower.startsWith('audio/')) return 'audio';
    if (lower.startsWith('video/')) return 'video';
    if (lower.startsWith('model/')) return '3d';
    if (lower.startsWith('image/')) return 'image';
    return 'binary';
};

const formatTimestamp = (seconds) => {
    if (!seconds) return '—';
    const date = new Date(seconds * 1000);
    return date.toISOString().replace('T', ' ').replace('Z', ' UTC');
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

const buildProgressBar = (value, tip) => {
    if (!tip || tip === 0) return '░░░░░░░░░░░░░░░░░░░░ 0%';
    const ratio = Math.max(0, Math.min(1, value / tip));
    const segments = 20;
    const filled = Math.round(ratio * segments);
    return `${'█'.repeat(filled)}${'░'.repeat(segments - filled)} ${(ratio * 100).toFixed(1)}%`;
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
            const tldAttr = this.getAttribute && this.getAttribute('tld');
            const tld = tldAttr ? `&tld=${encodeURIComponent(tldAttr)}` : '';
            const res = await fetch(`${this.endpoint}?page=${this.page}&limit=${this.limit}${q}${tld}`);
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
            const bucket = (item.category || kindFromContentType(item.content_type)).toLowerCase();
            return this.typeFilter === 'all' || bucket === this.typeFilter;
        });

        if (!filtered.length) {
            this.setPlaceholder('No inscriptions for selection', 'empty');
            return;
        }

        filtered.forEach((item) => {
            const kind = (item.category || kindFromContentType(item.content_type)).toLowerCase();
            const card = document.createElement('article');
            card.className = 'card';
            card.dataset.kind = kind;
            card.classList.add(item.shielded ? 'shielded' : 'transparent');

            const header = document.createElement('header');
            const idLink = document.createElement('a');
            idLink.href = `/inscription/${item.id}`;
            idLink.textContent = item.id.slice(0, 12) + '…';
            header.appendChild(idLink);

            const typeTag = document.createElement('span');
            typeTag.textContent = kind.toUpperCase();
            header.appendChild(typeTag);
            card.appendChild(header);

            const lowerType = (item.content_type || '').toLowerCase();
            const isJson = lowerType.includes('json');

            if (isJson) {
                const pre = document.createElement('pre');
                pre.textContent = 'loading json…';
                card.appendChild(pre);
                fetch(`/content/${item.id}`)
                    .then((resp) => resp.text())
                    .then((text) => {
                        try {
                            const pretty = JSON.stringify(JSON.parse(text), null, 2);
                            pre.textContent = pretty.slice(0, 1600);
                        } catch (err) {
                            pre.textContent = text.slice(0, 1600);
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
            addr.className = `badge ${item.shielded ? 'badge-shielded' : 'badge-transparent'}`;
            footer.appendChild(addr);

            const height = document.createElement('span');
            height.textContent = item.block_height ? `h${formatNumber(item.block_height)}` : '—';
            footer.appendChild(height);

            const size = document.createElement('span');
            size.textContent = formatBytes(item.content_length);
            footer.appendChild(size);

            const ts = document.createElement('span');
            ts.textContent = formatTimestamp(item.block_time);
            footer.appendChild(ts);

            const tx = document.createElement('span');
            tx.textContent = item.txid ? item.txid.slice(0, 10) + '…' : '—';
            footer.appendChild(tx);

            card.appendChild(footer);

            this.grid.appendChild(card);
        });
    }
}

class TokenTable extends PaginatedComponent {
    setup() {
        super.setup();
        this.asOf = document.createElement('div');
        this.asOf.className = 'notes';
        this.table = document.createElement('table');
        const thead = document.createElement('thead');
        thead.innerHTML = '<tr>' +
            '<th data-sort="ticker">Ticker</th>' +
            '<th data-sort="supply">Supply</th>' +
            '<th data-sort="max">Max&nbsp;Supply</th>' +
            '<th data-sort="limit">Mint&nbsp;Limit</th>' +
            '<th data-sort="progress">Progress</th>' +
            '<th>Inscription</th>' +
        '</tr>';
        thead.addEventListener('click', (e) => {
            const th = e.target.closest('th[data-sort]');
            if (!th) return;
            const key = th.dataset.sort;
            if (this.sortKey === key) {
                this.sortDir = this.sortDir === 'asc' ? 'desc' : 'asc';
            } else {
                this.sortKey = key;
                this.sortDir = 'desc';
            }
            this.render(this._lastItems || []);
            localStorage.setItem('zord_token_sort_key', this.sortKey);
            localStorage.setItem('zord_token_sort_dir', this.sortDir);
        });
        this.table.appendChild(thead);
        this.tbody = document.createElement('tbody');
        this.table.appendChild(this.tbody);
        this.container.appendChild(this.asOf);
        this.container.appendChild(this.table);
        this.sortKey = localStorage.getItem('zord_token_sort_key') || 'progress';
        this.sortDir = localStorage.getItem('zord_token_sort_dir') || 'desc';
    }

    // Format big base-unit string into human units using decimals (string-safe)
    formatUnits(baseStr, decimals) {
        const dec = Math.max(0, parseInt(decimals || '0', 10));
        let s = String(baseStr || '0').replace(/^0+/, '');
        if (!s) return '0';
        if (dec === 0) return s;
        if (s.length <= dec) {
            const pad = '0'.repeat(dec - s.length);
            return `0.${pad}${s}`.replace(/\.0+$/, '');
        }
        const head = s.slice(0, s.length - dec);
        const tail = s.slice(s.length - dec).replace(/0+$/, '');
        return tail ? `${head}.${tail}` : head;
    }

    get endpoint() {
        return '/api/v1/tokens';
    }

    render(items) {
        this._lastItems = Array.isArray(items) ? items.slice() : [];
        const list = this.sortItems(this._lastItems);
        this.tbody.innerHTML = '';
        if (!list.length) {
            this.setPlaceholder('No tokens deployed', 'empty');
            return;
        }

        this.container.innerHTML = '';
        this.container.appendChild(this.asOf);
        this.container.appendChild(this.table);
        // Update status banner
        fetch('/api/v1/status')
            .then((r) => r.ok ? r.json() : null)
            .then((status) => {
                if (!status) return;
                const h = status.height ?? '—';
                const tip = status.chain_tip ?? '—';
                this.asOf.textContent = `As of height ${h} (tip: ${tip})`;
            })
            .catch(() => {});
        list.forEach((token) => {
            const row = document.createElement('tr');
            const ticker = document.createElement('td');
            ticker.textContent = token.ticker.toUpperCase();
            row.appendChild(ticker);

            const supply = document.createElement('td');
            supply.textContent = token.supply;
            supply.title = `base: ${token.supply_base_units}, dec: ${token.dec}`;
            row.appendChild(supply);

            const max = document.createElement('td');
            const maxHuman = this.formatUnits(token.max_base_units, token.dec);
            max.textContent = maxHuman;
            max.title = `raw: ${token.max}, dec: ${token.dec}, base: ${token.max_base_units}`;
            row.appendChild(max);

            const lim = document.createElement('td');
            lim.textContent = token.lim;
            lim.title = `raw: ${token.lim}, dec: ${token.dec}`;
            row.appendChild(lim);

            const progressCell = document.createElement('td');
            const bar = document.createElement('div');
            bar.className = 'progress';
            const fill = document.createElement('span');
            fill.style.width = `${(token.progress * 100).toFixed(2)}%`;
            bar.appendChild(fill);
            bar.title = `progress = supply/max (${token.supply_base_units}/${token.max_base_units})`;
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

    sortItems(items) {
        const key = this.sortKey;
        const dir = this.sortDir === 'asc' ? 1 : -1;
        const toBig = (s) => { try { return BigInt(String(s||'0')); } catch { return 0n; } };
        const decToBig = (str, dec) => {
            const d = parseInt(dec || '0', 10);
            if (!str || str === '0') return 0n;
            if (String(str).includes('.')) {
                const [w, f=''] = String(str).split('.');
                const frac = (f + '0'.repeat(Math.max(0, d - f.length))).slice(0, d);
                return toBig(w) * (10n ** BigInt(d)) + toBig(frac);
            }
            return toBig(str) * (10n ** BigInt(d));
        };
        const get = (t) => {
            if (key === 'ticker') return t.ticker.toLowerCase();
            if (key === 'supply') return toBig(t.supply_base_units || '0');
            if (key === 'max') return toBig(t.max_base_units || '0');
            if (key === 'limit') return decToBig(t.lim || '0', t.dec || '0');
            if (key === 'progress') return Number(t.progress || 0);
            return 0;
        };
        const cmp = (a, b) => {
            const va = get(a), vb = get(b);
            if (typeof va === 'string' && typeof vb === 'string') return va.localeCompare(vb) * dir;
            if (typeof va === 'number' || typeof vb === 'number') return (Number(va) - Number(vb)) * dir;
            if (va === vb) return 0;
            return (va > vb ? 1 : -1) * dir;
        };
        return items.slice().sort(cmp);
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

    static get observedAttributes() { return ['search-query','tld','sort-dir']; }
    attributeChangedCallback(name, oldValue, newValue) {
        if (name === 'search-query' && oldValue !== newValue) {
            this.query = newValue;
            this.page = 0;
            this.fetchPage();
        }
        if ((name === 'tld' || name === 'sort-dir') && oldValue !== newValue) {
            this.page = 0;
            this.fetchPage();
        }
    }

    render(items) {
        const dir = (this.getAttribute('sort-dir') || 'desc').toLowerCase();
        const sorted = (items || []).slice().sort((a,b)=>{
            const na = (a.name || '').toLowerCase();
            const nb = (b.name || '').toLowerCase();
            const cmp = na.localeCompare(nb);
            return dir === 'asc' ? cmp : -cmp;
        });
        if (!sorted.length) {
            // Try to show syncing context instead of a blank/empty state during reindexing
            fetch('/api/v1/status')
                .then((r) => (r.ok ? r.json() : null))
                .then((status) => {
                    if (!status) {
                        this.setPlaceholder('No names available', 'empty');
                        return;
                    }
                    const h = status.components?.names?.height ?? status.height ?? '—';
                    const tip = status.chain_tip ?? '—';
                    this.setPlaceholder(`Syncing names… as of height ${h} (tip: ${tip})`, 'loading');
                })
                .catch(() => this.setPlaceholder('No names available', 'empty'));
            return;
        }

        this.container.innerHTML = '';
        this.container.appendChild(this.list);
        this.list.innerHTML = '';
        sorted.forEach((entry) => {
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
class SyncFooter extends HTMLElement {
    connectedCallback() {
        this.renderPlaceholder();
        this.refresh();
        this.timer = setInterval(() => this.refresh(), 8000);
    }

    disconnectedCallback() {
        clearInterval(this.timer);
    }

    renderPlaceholder() {
        this.className = 'sync-footer';
        this.textContent = 'syncing…';
    }

    async refresh() {
        try {
            const res = await fetch('/api/v1/status');
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            const data = await res.json();
            this.render(data);
        } catch (err) {
            this.textContent = 'status offline';
        }
    }

    render(data) {
        this.innerHTML = '';
        const components = data.components || {};
        const entries = [
            ['core', components.core || {}],
            ['zrc20', components.zrc20 || {}],
            ['names', components.names || {}],
        ];
        entries.forEach(([label, info]) => {
            const seg = document.createElement('div');
            seg.className = 'sync-segment';
            const bar = document.createElement('span');
            bar.className = 'sync-bar';
            bar.textContent = buildProgressBar(info.height || 0, info.tip || 0);
            const text = document.createElement('span');
            text.textContent = label;
            seg.appendChild(text);
            seg.appendChild(bar);
            this.appendChild(seg);
        });
    }
}
customElements.define('sync-footer', SyncFooter);

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

    const chipRow = document.getElementById('type-filter');
    if (chipRow) {
        chipRow.addEventListener('click', (event) => {
            const button = event.target.closest('button[data-kind]');
            if (!button) return;
            chipRow.querySelectorAll('.chip').forEach((chip) => chip.classList.remove('active'));
            button.classList.add('active');
            const feed = document.querySelector('inscription-feed');
            if (feed && typeof feed.setTypeFilter === 'function') {
                feed.setTypeFilter(button.dataset.kind);
            }
        });
    }
})();
