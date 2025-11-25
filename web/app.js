const targetSelectors = {
    inscriptions: 'inscription-feed',
    tokens: 'token-table',
    names: 'name-table',
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
        this.grid.className = 'z-grid';
        this.container.appendChild(this.grid);
    }

    get endpoint() {
        return '/api/v1/inscriptions';
    }

    render(items) {
        this.grid.innerHTML = '';
        if (!items.length) {
            this.setPlaceholder('No inscriptions yet', 'empty');
            return;
        }
        items.forEach((item) => {
            const card = document.createElement('article');
            card.className = 'z-card';

            const body = document.createElement('p');
            if (item.preview_text) {
                body.textContent = item.preview_text;
            } else if (item.content_type.startsWith('image/')) {
                body.textContent = 'Image inscription';
            } else {
                body.textContent = `${item.content_type} · ${item.content_length} bytes`;
            }
            card.appendChild(body);

            const meta = document.createElement('div');
            meta.className = 'z-grid-meta';
            const idLink = document.createElement('a');
            idLink.href = `/inscription/${item.id}`;
            idLink.textContent = `${item.id.slice(0, 8)}…`;
            meta.appendChild(idLink);

            const type = document.createElement('span');
            type.textContent = item.content_type;
            meta.appendChild(type);
            card.appendChild(meta);

            const sender = document.createElement('p');
            sender.textContent = `from ${item.sender.slice(0, 12)}…`;
            card.appendChild(sender);

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
        items.forEach((token) => {
            const row = document.createElement('tr');
            const ticker = document.createElement('td');
            ticker.textContent = token.ticker;
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
        this.list.innerHTML = '';
        if (!items.length) {
            this.setPlaceholder('No names registered', 'empty');
            return;
        }
        items.forEach((entry) => {
            const li = document.createElement('li');
            const name = document.createElement('strong');
            name.textContent = entry.name;
            li.appendChild(name);

            const owner = document.createElement('span');
            owner.textContent = entry.owner.slice(0, 12) + '…';
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
})();
