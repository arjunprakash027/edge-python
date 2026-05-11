/* 
Syntax highlighter: single-pass tokenizer emitting <span class='tk-...'> markup; f-strings recurse for `{expr}`. 
*/

export const Highlighter = (() => {
    const KW = new Set(['as','if','in','is','or','and','def','del','for','not','try','case','elif','else','from','pass','type','with','async','await','break','class','match','raise','while','yield','assert','except','global','import','lambda','return','finally','continue','nonlocal']);
    const BI = new Set(['print','len','range','int','str','float','list','dict','tuple','set','bool','isinstance','enumerate','zip','abs','min','max','sum','round','pow','divmod','hash','id','repr','ord','chr','hex','oct','bin','input','reversed','sorted','any','all','format','ascii','callable','getattr','hasattr','type','self','cls']);
    const LIT = new Set(['True', 'False', 'None']);
    const CLASSES = [[KW, 'tk-kw'], [LIT, 'tk-lit'], [BI, 'tk-bi']];

    // Both compiled once; `.replace(re, fn)` resets `lastIndex` per call, making recursive f-string re-entry safe.
    const TOKEN_RE = /(#[^\n]*)|((?:\b[fFrRbBuU]{1,2})?(?:"""[\s\S]*?"""|'''[\s\S]*?'''|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'))|(0[xX][\da-fA-F_]+|0[oO][0-7_]+|0[bB][01_]+|\d[\d_]*(?:\.[\d_]*)?(?:[eE][+-]?\d+)?[jJ]?|\.\d[\d_]*(?:[eE][+-]?\d+)?[jJ]?)|([A-Za-z_]\w*)/g;
    const F_INNER_RE = /\{\{|\}\}|\{([^{}]*)\}/g;
    const ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;' };
    const esc = (s) => s.replace(/[&<>]/g, (c) => ESC[c]);
    const span = (cls, s) => `<span class="${cls}">${s}</span>`;

    const tokenize = (m, com, str, num, word, offset, full) => {
        if (com) return span('tk-com', com);
        if (str) {
            if (!/^[fFrRbBuU]*[fF]/.test(str)) return span('tk-str', str);
            // f-string: alternate `tk-str` with `tk-fexpr` for `{expr}`; recurse TOKEN_RE inside so identifiers highlight.
            const parts = [];
            let last = 0;
            str.replace(F_INNER_RE, (match, expr, off) => {
                if (last < off) parts.push(span('tk-str', str.slice(last, off)));
                parts.push(expr != null
                    ? span('tk-fexpr', `{${expr.replace(TOKEN_RE, tokenize)}}`)
                    : span('tk-str', match));
                last = off + match.length;
                return '';
            });
            if (last < str.length) parts.push(span('tk-str', str.slice(last)));
            return parts.join('');
        }
        if (num) return span('tk-num', num);
        if (word) {
            // Skip styling inside HTML entities (`&amp;` etc.): `esc()` wrote them, so `amp` shouldn't render as variable.
            if (full[offset - 1] === '&' && full[offset + word.length] === ';') return word;
            for (const [set, cls] of CLASSES) if (set.has(word)) return span(cls, word);
            if (/^[A-Z]/.test(word)) return span('tk-class', word);
            return span(/^\s*\(/.test(full.slice(offset + word.length)) ? 'tk-func' : 'tk-var', word);
        }
        return m;
    };

    return { highlight: (src) => esc(src).replace(TOKEN_RE, tokenize) };
})();
