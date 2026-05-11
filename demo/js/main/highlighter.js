/* Python syntax highlighter.
   Single-pass tokenizer that emits inline <span class="tk-..."> markup.
   F-string interiors recurse so `{expr}` interpolations are highlighted as code. */

export const Highlighter = (() => {
    const KW = new Set(['as','if','in','is','or','and','def','del','for','not','try','case','elif','else','from','pass','type','with','async','await','break','class','match','raise','while','yield','assert','except','global','import','lambda','return','finally','continue','nonlocal']);
    const BI = new Set(['print','len','range','int','str','float','list','dict','tuple','set','bool','isinstance','enumerate','zip','abs','min','max','sum','round','pow','divmod','hash','id','repr','ord','chr','hex','oct','bin','input','reversed','sorted','any','all','format','ascii','callable','getattr','hasattr','type','self','cls']);
    const LIT = new Set(['True', 'False', 'None']);
    const CLASSES = [[KW, 'tk-kw'], [LIT, 'tk-lit'], [BI, 'tk-bi']];

    const TOKEN_RE = /(#[^\n]*)|((?:\b[fFrRbBuU]{1,2})?(?:"""[\s\S]*?"""|'''[\s\S]*?'''|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'))|(0[xX][\da-fA-F_]+|0[oO][0-7_]+|0[bB][01_]+|\d[\d_]*(?:\.[\d_]*)?(?:[eE][+-]?\d+)?[jJ]?|\.\d[\d_]*(?:[eE][+-]?\d+)?[jJ]?)|([A-Za-z_]\w*)/g;
    // Pre-compiled once; `replace` resets lastIndex per call so re-entry is safe.
    const F_INNER_RE = /\{\{|\}\}|\{([^{}]*)\}/g;
    const ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;' };
    const esc = (s) => s.replace(/[&<>]/g, (c) => ESC[c]);
    const span = (cls, s) => `<span class="${cls}">${s}</span>`;

    const tokenize = (m, com, str, num, word, offset, full) => {
        if (com) return span('tk-com', com);
        if (str) {
            if (/^[fFrRbBuU]*[fF]/.test(str)) {
                const parts = [];
                let last = 0;
                F_INNER_RE.lastIndex = 0;
                let fm;
                while ((fm = F_INNER_RE.exec(str)) !== null) {
                    if (last < fm.index) parts.push(span('tk-str', str.slice(last, fm.index)));
                    if (fm[1] != null) {
                        parts.push(span('tk-fexpr', `{${fm[1].replace(TOKEN_RE, tokenize)}}`));
                    } else {
                        parts.push(span('tk-str', fm[0]));
                    }
                    last = F_INNER_RE.lastIndex;
                }
                if (last < str.length) parts.push(span('tk-str', str.slice(last)));
                return parts.join('');
            }
            return span('tk-str', str);
        }
        if (num) return span('tk-num', num);
        if (word) {
            if (full[offset - 1] === '&' && full[offset + word.length] === ';') return word;
            for (const [set, cls] of CLASSES) if (set.has(word)) return span(cls, word);
            if (/^[A-Z]/.test(word)) return span('tk-class', word);
            return span(/^\s*\(/.test(full.slice(offset + word.length)) ? 'tk-func' : 'tk-var', word);
        }
        return m;
    };

    return { highlight: (src) => esc(src).replace(TOKEN_RE, tokenize) };
})();
