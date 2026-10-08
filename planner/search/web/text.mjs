import addressTerms from './address-terms.json' with {type:'json'};

export function norm(s) {
  return String(s).toLowerCase().replaceAll('ß','ss').normalize('NFKD')
    .replace(/\p{M}/gu,'').replace(/[^\p{L}\p{N}]+/gu,' ').trim();
}
export const compact=s=>norm(s).replaceAll(' ','');

function dictionary(rows) {
  const entries=new Map();
  for(const row of rows)for(const word of row) {
    const key=norm(word),canonical=norm(row[0]);
    if(entries.has(key)&&entries.get(key)!==canonical)entries.set(key,null);
    else entries.set(key,canonical);
  }
  return entries;
}
const words=dictionary(addressTerms.words);
const suffixes=[...dictionary(addressTerms.suffixes)].filter(([k,v])=>k.length>=2&&v)
  .sort((a,b)=>b[0].length-a[0].length);

// Apply address language knowledge only when comparing or indexing street names.
export function streetNorm(s) {
  return norm(s).split(' ').map(word=>{
    if(words.get(word))return words.get(word);
    for(const [suffix,full]of suffixes)
      if(word.length>suffix.length+1&&word.endsWith(suffix))return word.slice(0,-suffix.length)+' '+full;
    return word;
  }).join(' ');
}

export function editDistance(a,b) {
  let row=Array.from({length:b.length+1},(_,i)=>i),previous;
  for(let i=0;i<a.length;i++) {
    const next=[i+1];
    for(let j=0;j<b.length;j++) {
      let value=Math.min(next[j]+1,row[j+1]+1,row[j]+(a[i]!==b[j]));
      if(i>0&&j>0&&a[i]===b[j-1]&&a[i-1]===b[j])value=Math.min(value,previous[j-1]+1);
      next.push(value);
    }
    previous=row;row=next;
  }
  return row[b.length];
}

export const editBudget=s=>s.length<5?0:s.length<9?1:2;
export function spans(q) {
  const tokens=norm(q).split(' ').slice(0,8),out=[];
  for(let a=0;a<tokens.length;a++)for(let b=a+1;b<=tokens.length;b++) {
    const term=tokens.slice(a,b).join('');
    out.push({term,replace:t=>[...tokens.slice(0,a),t,...tokens.slice(b)].join(' ')});
  }
  return out;
}
