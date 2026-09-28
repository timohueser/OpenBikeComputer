import {test} from 'node:test';
import assert from 'node:assert/strict';
import {routeResults} from '../web/route-results.mjs';

test('the first page covers the route despite a dense start and more retains earlier choices', () => {
  const rows = Array.from({length:100}, (_,i) => ({source:`start-${i}`, position:{along:i/100, distance:0}}));
  for (let i=5;i<=100;i+=5) rows.push({source:`route-${i}`, position:{along:i, distance:0.05}});
  const first = routeResults(rows,[0,100],20);
  const more = routeResults([...rows].reverse(),[0,100],40);
  assert.equal(first.length,20);
  assert.ok(first[0].position.along < 10);
  assert.ok(first.at(-1).position.along > 90);
  assert.ok(first.filter(p=>p.position.along<1).length<=2);
  assert.ok(first.every(p=>more.some(q=>p.source===q.source)));
  assert.ok(first.every((p,i)=>!i || p.position.along>=first[i-1].position.along));
});

test('coverage favours proximity when route positions are close', () => {
  const rows = [
    {source:'far',position:{along:50,distance:.9}},
    {source:'near',position:{along:50.1,distance:.01}},
  ];
  assert.equal(routeResults(rows,[0,100],1)[0].source,'near');
});
