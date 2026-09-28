"""Hand-tagged cases for decode.py, and a few for rules.py. Markup: "[words|SLOT]"."""

import re

import pytest

import rules
import schema
import words
from decode import (Span, decode, name_of, parse_along, parse_point, parse_stretch, parse_what,
                    quantity)


def tag(markup: str) -> tuple[str, list[str]]:
    """Plain text and BIO labels from "add [water|POINT] [stops|KIND]"."""
    text, spans, pos = "", [], 0
    for m in re.finditer(r"\[([^\]|]+)\|([A-Z_]+)\]", markup):
        text += markup[pos:m.start()]
        spans.append((len(text), len(text) + len(m.group(1)), m.group(2)))
        text += m.group(1)
        pos = m.end()
    text += markup[pos:]
    labels = []
    for _, s, e in words.split(text):
        lab = "O"
        for a, b, slot in spans:
            if a <= s and e <= b:
                lab = ("B-" if s == a else "I-") + slot
        labels.append(lab)
    return text, labels


def km(v):
    return {"value": v, "unit": "km"}


def h(v):
    return {"value": v, "unit": "h"}


CASES = [
    # English
    ("add_point", "add [water|POINT] [stops|KIND] [every three hours|EVERY] to [route|SCOPE]",
     {"type": "add_point", "point": {"kind": "water"}, "kind": "stop", "every": h(3)}),
    ("places", "[supermarket|WHAT] [within the first 20 kilometers|ALONG]",
     {"type": "places", "what": ["supermarket"], "where": {"along": {"ref": "km", "to": km(20)}}}),
    ("route", "route to [the nearest pharmacy|TO]",
     {"type": "route", "to": {"kind": "pharmacy"}}),
    ("route", "take me up [the Furka pass|TO]",
     {"type": "route", "to": {"name": "Furka pass"}}),
    ("route", "[Basel|FROM] to [Nice|TO] [in 7 days|DAYS]",
     {"type": "route", "from": {"name": "Basel"}, "to": {"name": "Nice"}, "days": 7}),
    ("places", "last [water|WHAT] before [the Stelvio|BEFORE]",
     {"type": "places", "what": ["water"], "where": {"before": {"name": "Stelvio"}}}),
    ("end_day", "end [every day|DAY] at [a campsite|POINT]",
     {"type": "end_day", "day": "every", "at": {"kind": "campsite"}}),
    ("reroute", "make [day 3|DAY] [flatter|GOAL]",
     {"type": "reroute", "where": {"day": 3}, "goal": "least_climbing"}),
    ("split", "split into days of [about 80 km|PER_DAY]",
     {"type": "split", "per_day": km(80)}),
    ("places", "[campsites|WHAT] [with a pool|IGNORED] [end of day 4|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": 4, "part": "end"},
      "ignored": ["with a pool"]}),
    ("stretches", "[water gaps|WHAT]", {"type": "stretches", "what": "gap:water"}),
    ("stretches", "[longest stretch without a shop|WHAT] on [day 2|DAY]",
     {"type": "stretches", "what": "gap:resupply", "where": {"day": 2}}),
    ("stretches", "[climbs|WHAT] [steeper than 8 %|MIN] [tomorrow|DAY]",
     {"type": "stretches", "what": "climb", "min": {"value": 8, "unit": "%"},
      "where": {"day": "tomorrow"}}),
    ("place", "[Kandel|NAME]", {"type": "place", "name": "Kandel"}),
    ("place", "[Hotel Krone|NAME] near [Freiburg|NEAR]",
     {"type": "place", "name": "Hotel Krone", "near": {"name": "Freiburg"}}),
    ("add_point", "via [Hotel Krone St. Peter|POINT]",
     {"type": "add_point", "point": {"name": "Hotel Krone St. Peter"}}),
    ("remove_point", "remove the stop at [Kandel|POINT]",
     {"type": "remove_point", "point": {"name": "Kandel"}}),
    ("join", "join [days 3 and 4|DAY]", {"type": "join", "day": 3}),
    ("reverse", "reverse the route", {"type": "reverse"}),
    ("places", "[shops|WHAT] [open on sunday|OPEN] near [me|NEAR]",
     {"type": "places", "what": ["resupply"], "open": {"weekday": "sun"},
      "where": {"scope": "here"}}),
    ("places", "[shops|WHAT] [open|OPEN] [day 3|DAY]",
     {"type": "places", "what": ["resupply"], "open": {"day": 3}, "where": {"day": 3}}),
    ("places", "[cafes|WHAT] in [the next 2 hours|ALONG]",
     {"type": "places", "what": ["cafe"], "where": {"along": {"ref": "here", "to": h(2)}}}),
    ("places", "[water|WHAT] at [km 40 to 60|ALONG]",
     {"type": "places", "what": ["water"],
      "where": {"along": {"ref": "km", "from": km(40), "to": km(60)}}}),
    ("places", "[bakeries|WHAT] [within 3 km|RADIUS] of [the route|SCOPE]",
     {"type": "places", "what": ["bakery"], "radius": km(3), "where": {"scope": "route"}}),
    ("route", "[gravel|BIKE] route from [here|FROM] to [Titisee|TO], [least climbing|GOAL]",
     {"type": "route", "from": {"here": True}, "to": {"name": "Titisee"}, "bike": "gravel",
      "goal": "least_climbing"}),
    ("route", "[Munich|FROM] to [Venice|TO], [100 km a day|PER_DAY]",
     {"type": "route", "from": {"name": "Munich"}, "to": {"name": "Venice"},
      "per_day": km(100)}),
    ("end_day", "end [day 4|DAY] at [saint-michel|POINT]",
     {"type": "end_day", "day": 4, "at": {"name": "Saint-Michel"}}),
    ("none", "is the galibier worth it", {"type": "none"}),
    ("split", "split [day 3|DAY]", {"type": "split", "days": 2, "where": {"day": 3}}),
    ("add_point", "add a [stop|KIND] at [km 120|POINT]",
     {"type": "add_point", "point": {"along": {"ref": "km", "at": km(120)}}, "kind": "stop"}),
    ("places", "[pharmcies|WHAT] [along the route|SCOPE]",
     {"type": "places", "what": ["pharmacy"], "where": {"scope": "route"}}),
    ("places", "[flurbs|WHAT] on [day 2|DAY]", {"type": "none"}),
    # German
    ("places", "wo kann ich am [Ende von Tag 4|DAY] [zelten|WHAT]",
     {"type": "places", "what": ["campsite"], "where": {"day": 4, "part": "end"}}),
    ("places", "[Campingplatze|WHAT] [Ende Tag vier|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": 4, "part": "end"}}),
    ("places", "[Supermärkte|WHAT] [morgen Mittag|DAY]",
     {"type": "places", "what": ["supermarket"], "where": {"day": "tomorrow", "part": "middle"}}),
    ("route", "Route nach [Basel|TO] über [Freiburg|VIA] mit dem [Rennrad|BIKE]",
     {"type": "route", "to": {"name": "Basel"}, "via": [{"name": "Freiburg"}], "bike": "road"}),
    ("join", "[Tag 3|DAY] und [4|DAY] zusammenlegen", {"type": "join", "day": 3}),
    ("add_point", "[Wasser|POINT] [alle 50 km|EVERY] einplanen",
     {"type": "add_point", "point": {"kind": "water"}, "every": km(50)}),
    ("places", "[Apotheke|WHAT] [sonntags geöffnet|OPEN] in [Freiburg|NEAR]",
     {"type": "places", "what": ["pharmacy"], "open": {"weekday": "sun"},
      "where": {"near": [{"name": "Freiburg"}]}}),
    ("reroute", "[Tag 2|DAY] [ohne Schotter|GOAL]",
     {"type": "reroute", "where": {"day": 2}, "goal": "least_unpaved"}),
    ("stretches", "[Anstiege|WHAT] [über 500 Höhenmeter|MIN]",
     {"type": "stretches", "what": "climb", "min": {"value": 500, "unit": "m"}}),
    ("stretches", "[Lücken ohne Wasser|WHAT] in [den nächsten dreißig Kilometern|ALONG]",
     {"type": "stretches", "what": "gap:water", "where": {"along": {"ref": "here", "to": km(30)}}}),
    ("split", "in [fünf Tage|DAYS] aufteilen", {"type": "split", "days": 5}),
    ("places", "[Brunnen|WHAT] [in der Nähe|SCOPE]",
     {"type": "places", "what": ["fountain"], "where": {"scope": "here"}}),
    # French
    ("places", "[supermarché|WHAT] [ouvert dimanche|OPEN] [jour 3|DAY]",
     {"type": "places", "what": ["supermarket"], "open": {"weekday": "sun"}, "where": {"day": 3}}),
    ("places", "où [dormir|WHAT] [ce soir|DAY]",
     {"type": "places", "what": ["sleep"], "where": {"day": "today", "part": "end"}}),
    ("route", "itinéraire vers [le col du Galibier|TO] en [VTT|BIKE]",
     {"type": "route", "to": {"name": "col du Galibier"}, "bike": "mtb"}),
    ("end_day", "terminer le [jour 2|DAY] à [Briançon|POINT]",
     {"type": "end_day", "day": 2, "at": {"name": "Briançon"}}),
    ("places", "[eau|WHAT] dans [les 20 derniers km|ALONG]",
     {"type": "places", "what": ["water"], "where": {"along": {"ref": "end", "to": km(20)}}}),
    ("add_point", "ajoute une [boulangerie|POINT] [comme arrêt|KIND] au [deuxième jour|DAY]",
     {"type": "add_point", "point": {"kind": "bakery"}, "kind": "stop", "where": {"day": 2}}),
    # Italian
    ("places", "[campeggi|WHAT] alla [fine del giorno 4|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": 4, "part": "end"}}),
    ("route", "percorso da [qui|FROM] al [Passo dello Stelvio|TO] in [bici da corsa|BIKE]",
     {"type": "route", "from": {"here": True}, "to": {"name": "Passo dello Stelvio"},
      "bike": "road"}),
    ("stretches", "[tratti sterrati|WHAT] nel [giorno 2|DAY]",
     {"type": "stretches", "what": "unpaved", "where": {"day": 2}}),
    ("places", "[supermercato|WHAT] [entro 500 m|RADIUS]",
     {"type": "places", "what": ["supermarket"], "radius": km(0.5)}),
    ("places", "[rifugi|WHAT] tra [Bormio|NEAR] e [Livigno|NEAR]",
     {"type": "places", "what": ["hut"],
      "where": {"near": [{"name": "Bormio"}, {"name": "Livigno"}]}}),
]


@pytest.mark.parametrize("intent,markup,gold", CASES, ids=[c[1] for c in CASES])
def test_decode(intent, markup, gold):
    text, labels = tag(markup)
    got = decode(text, intent, labels)
    schema.validate(got)
    assert schema.same(got, gold), got


@pytest.mark.parametrize("text,gold", [
    ("the nearest pharmacy", {"kind": "pharmacy"}),
    ("la pharmacie la plus proche", {"kind": "pharmacy"}),
    ("my location", {"here": True}),
    ("mir", {"here": True}),
    ("the end of day 3", {"day": 3, "part": "end"}),
    ("the start", {"plan": "start"}),
    ("km 120", {"along": {"ref": "km", "at": km(120)}}),
    ("Bern", {"name": "Bern"}),
    ("l'Alpe d'Huez", {"name": "Alpe d'Huez"}),
])
def test_point(text, gold):
    assert parse_point(text)[0] == gold


@pytest.mark.parametrize("text,dims,gold", [
    ("one and a half hours", ("km", "h"), h(1.5)),
    ("vingt-cinq km", ("km", "h"), km(25)),
    ("90 minutes", ("km", "h"), h(1.5)),
    ("1h30", ("km", "h"), h(1.5)),
    ("ventidue chilometri", ("km", "h"), km(22)),
    ("500 m", ("m", "km", "%"), {"value": 500, "unit": "m"}),
])
def test_quantity(text, dims, gold):
    assert quantity(Span(text), dims) == gold


def test_along_after_is_a_mark():
    assert parse_along(Span("after 100 km")) == {"ref": "start", "at": km(100)}


@pytest.mark.parametrize("text,gold", [
    ("campsites end of day 4",
     {"type": "places", "what": ["campsite"], "where": {"day": 4, "part": "end"}}),
    ("route to the nearest pharmacy", {"type": "route", "to": {"kind": "pharmacy"}}),
    ("Basel to Nice in 7 days",
     {"type": "route", "from": {"name": "Basel"}, "to": {"name": "Nice"}, "days": 7}),
    ("supermarché ouvert dimanche jour 3",
     {"type": "places", "what": ["supermarket"], "open": {"weekday": "sun"}, "where": {"day": 3}}),
    ("wo kann ich am Ende von Tag 4 zelten",
     {"type": "places", "what": ["campsite"], "where": {"day": 4, "part": "end"}}),
    ("make day 3 flatter", {"type": "reroute", "where": {"day": 3}, "goal": "least_climbing"}),
    ("water gaps", {"type": "stretches", "what": "gap:water"}),
    ("end day 4 at saint-michel", {"type": "end_day", "day": 4, "at": {"name": "saint-michel"}}),
    ("Kandel", {"type": "place", "name": "Kandel"}),
    ("is the galibier worth it", {"type": "none"}),
])
def test_rules(text, gold):
    got = rules.parse(text)
    schema.validate(got)
    assert schema.same(got, gold), got


# --- round 2: gaps found by the generator's filter pass ----------------------------------------

@pytest.mark.parametrize("text,kind", [
    ("bread shops", "bakery"), ("bike repair shops", "bike_shop"), ("refill points", "water"),
    ("water stops", "water"), ("places to eat", "food"), ("eateries", "restaurant"),
    ("places to camp", "campsite"), ("grocery stores", "supermarket"), ("eat out", "food"),
    ("GPs", "doctor"), ("train stops", "train_station"), ("transport", "transport"),
    ("galleries", "museum"), ("drinking fountains", "drinking_water"),
    ("kebab shops", "fast_food"), ("fix my bike", "bike"), ("fill my bottles", "water"),
    ("take a shower", "shower"), ("take public transport", "transport"), ("pee", "toilets"),
    ("Versorgungspunkte", "resupply"), ("Landgasthöfe", "restaurant"), ("Gasthöfe", "restaurant"),
    ("Duschmöglichkeiten", "shower"), ("Nachfüllstellen", "water"), ("Radhändler", "bike_shop"),
    ("Züge", "train_station"), ("Lokale", "restaurant"), ("Klos", "toilets"),
    ("Verpflegung", "resupply"), ("Trinkbrunnen", "drinking_water"), ("Galerien", "museum"),
    ("essen gehen", "food"), ("Brötchen holen", "bakery"), ("mein Rad reparieren", "bike"),
    ("mit dem Zug weiterfahren", "train_station"), ("où manger", "food"),
    ("réparer mon vélo", "bike"), ("faire pipi", "toilets"), ("prendre le train", "train_station"),
    ("posti per mangiare", "food"), ("riparare la bici", "bike"),
    ("fermata del treno", "train_station"), ("galleria d'arte", "museum"),
])
def test_kind_terms(text, kind):
    sp = Span(text)
    assert parse_what(sp) == [kind] and not sp.leftover()


@pytest.mark.parametrize("text,stretch", [
    ("Rampen", "steep"), ("Abfahrtsstrecken", "descent"), ("Vollsperrungen", "closure"),
    ("Wasserlücke", "gap:water"), ("Versorgungslücke", "gap:resupply"),
    ("Einkaufslücken", "gap:resupply"), ("manque d'eau", "gap:water"),
])
def test_stretch_terms(text, stretch):
    sp = Span(text)
    assert parse_stretch(sp) == stretch and not sp.leftover()


FILLER = [
    ("reroute", "[as flat as possible|GOAL] on [day 2|DAY]",
     {"type": "reroute", "where": {"day": 2}, "goal": "least_climbing"}),
    ("places", "[water|WHAT] [along my trip|SCOPE]",
     {"type": "places", "what": ["water"], "where": {"scope": "route"}}),
    ("places", "[cafes|WHAT] [halfway through day 3|DAY]",
     {"type": "places", "what": ["cafe"], "where": {"day": 3, "part": "middle"}}),
    ("route", "route to [Bern|TO] on [paved roads|GOAL]",
     {"type": "route", "to": {"name": "Bern"}, "goal": "least_unpaved"}),
    ("places", "[toilets|WHAT] [on my screen|SCOPE]",
     {"type": "places", "what": ["toilets"], "where": {"scope": "view"}}),
    ("split", "split [the whole trip|SCOPE] into [80 km daily|PER_DAY]",
     {"type": "split", "per_day": km(80)}),
    ("stretches", "[climbs|WHAT] [of 2 km or more|MIN]",
     {"type": "stretches", "what": "climb", "min": km(2)}),
    ("route", "[the most direct way|GOAL] to [Chur|TO]",
     {"type": "route", "to": {"name": "Chur"}, "goal": "shortest"}),
    ("route", "nach [Bern|TO] [auf dem kürzesten Weg|GOAL]",
     {"type": "route", "to": {"name": "Bern"}, "goal": "shortest"}),
    ("reroute", "[Tag 2|DAY] [so flach wie möglich|GOAL]",
     {"type": "reroute", "where": {"day": 2}, "goal": "least_climbing"}),
    ("reroute", "[Anstiege vermeiden|GOAL]", {"type": "reroute", "where": {"scope": "route"},
                                              "goal": "least_climbing"}),
    ("reroute", "[nur asphaltierte Straßen|GOAL]",
     {"type": "reroute", "where": {"scope": "route"}, "goal": "least_unpaved"}),
    ("reroute", "[ohne Feldwege|GOAL]",
     {"type": "reroute", "where": {"scope": "route"}, "goal": "least_unpaved"}),
    ("places", "[Hütten|WHAT] [entlang meiner Route|SCOPE]",
     {"type": "places", "what": ["hut"], "where": {"scope": "route"}}),
    ("places", "[Bäcker|WHAT] [hier in der Gegend|SCOPE]",
     {"type": "places", "what": ["bakery"], "where": {"scope": "here"}}),
    ("places", "[Zeltplätze|WHAT] am [Etappenziel von Tag 3|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": 3, "part": "end"}}),
    ("places", "[Apotheke|WHAT] [nicht weiter als 5 km|RADIUS]",
     {"type": "places", "what": ["pharmacy"], "radius": km(5)}),
    ("stretches", "[Anstiege|WHAT] [steiler als 10 %|MIN]",
     {"type": "stretches", "what": "climb", "min": {"value": 10, "unit": "%"}}),
    ("places", "[Wasser|WHAT] [genau hier|NEAR]",
     {"type": "places", "what": ["water"], "where": {"scope": "here"}}),
    ("places", "[Bäckerei|WHAT] [Mo geöffnet|OPEN]",
     {"type": "places", "what": ["bakery"], "open": {"weekday": "mon"}}),
    ("places", "[I need|IGNORED] [a campsite|WHAT]", {"type": "places", "what": ["campsite"]}),
    ("places", "[ho bisogno di una farmacia|WHAT]", {"type": "places", "what": ["pharmacy"]}),
]

DECODER_GAPS = [
    ("places", "[campsites|WHAT] [every morning|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": "every", "part": "start"}}),
    ("places", "[Bäcker|WHAT] am [Morgen von Tag 4|DAY]",
     {"type": "places", "what": ["bakery"], "where": {"day": 4, "part": "start"}}),
    ("places", "[water|WHAT] on [day five|DAY]",
     {"type": "places", "what": ["water"], "where": {"day": 5}}),
    ("places", "[cafés|WHAT] in [the next hour|ALONG]",
     {"type": "places", "what": ["cafe"], "where": {"along": {"ref": "here", "to": h(1)}}}),
    ("route", "[7-day tour|DAYS] to [Nice|TO]", {"type": "route", "to": {"name": "Nice"},
                                                 "days": 7}),
    ("route", "[Viertagestour|DAYS] nach [Wien|TO]",
     {"type": "route", "to": {"name": "Wien"}, "days": 4}),
    ("route", "[5-tägige|DAYS] Tour nach [Wien|TO]",
     {"type": "route", "to": {"name": "Wien"}, "days": 5}),
    ("split", "in [100 km/Tag|PER_DAY] aufteilen", {"type": "split", "per_day": km(100)}),
    ("route", "route from [the starting point|FROM] to [Basel|TO]",
     {"type": "route", "from": {"plan": "start"}, "to": {"name": "Basel"}}),
    ("route", "route to [Sandwich|TO]", {"type": "route", "to": {"name": "Sandwich"}}),
    ("route", "route to [Pharmacy|TO]", {"type": "route", "to": {"kind": "pharmacy"}}),
    ("places", "[Supermarkt|WHAT] [in meiner Nähe|NEAR]",
     {"type": "places", "what": ["supermarket"], "where": {"scope": "here"}}),
    ("places", "[tonight's|DAY] [campsites|WHAT]",
     {"type": "places", "what": ["campsite"], "where": {"day": "today", "part": "end"}}),
    ("join", "merge [day 4|DAY] into [day 3|DAY]", {"type": "join", "day": 3}),
    ("add_point", "[Wasserstopps|POINT] [alle 2 Stunden|EVERY]",
     {"type": "add_point", "point": {"kind": "water"}, "every": h(2), "kind": "stop"}),
    ("add_point", "[Kaffeepause|POINT] an [Tag 2|DAY]",
     {"type": "add_point", "point": {"kind": "cafe"}, "kind": "stop", "where": {"day": 2}}),
    ("add_point", "add [water stops|POINT] at [km 50|ALONG]",
     {"type": "add_point", "point": {"kind": "water"}, "kind": "stop",
      "where": {"along": {"ref": "km", "at": km(50)}}}),
    ("route", "[Rennradtour|BIKE] nach [Basel|TO]",
     {"type": "route", "to": {"name": "Basel"}, "bike": "road"}),
    ("places", "[water and campsites|WHAT] on [day 2|DAY]",
     {"type": "places", "what": ["water", "campsite"], "where": {"day": 2}}),
    ("places", "[Trinkwasser, Betten und Kaffeehäuser|WHAT]",
     {"type": "places", "what": ["drinking_water", "lodging", "cafe"]}),
    ("places", "[water|WHAT] and [campsites|WHAT]",
     {"type": "places", "what": ["water", "campsite"]}),
    ("stretches", "[gaps|WHAT] in [water|WHAT] supply",
     {"type": "stretches", "what": "gap:water"}),
]


@pytest.mark.parametrize("intent,markup,gold", FILLER + DECODER_GAPS,
                         ids=[c[1] for c in FILLER + DECODER_GAPS])
def test_round2(intent, markup, gold):
    test_decode(intent, markup, gold)


# --- round 3: determinism, FR/IT gaps, elided names --------------------------------------------

def test_same_output_under_any_hash_seed():
    import os
    import subprocess
    import sys
    script = ("import json, rules; print(json.dumps([rules.parse(t) for t in "
              "['water gaps', 'Épiceries le soir du jour 10', 'pharmcy near me', "
              "'dove posso prendere un caffè', 'Tours panoramiques au km 230', "
              "'Wasserlücken auf Tag 2', 'supermakret morgen Mittag']], sort_keys=True))")
    outs = {subprocess.run([sys.executable, "-c", script], capture_output=True, text=True,
                           env={**os.environ, "PYTHONHASHSEED": seed}, check=True).stdout
            for seed in ("0", "1", "2")}
    assert len(outs) == 1, outs


ROUND3 = [
    # kinds, aligned to the generator's taxonomy
    ("places", "[auberges|WHAT] [à proximité|SCOPE]",
     {"type": "places", "what": ["hotel"], "where": {"scope": "here"}}),
    ("places", "[épiceries|WHAT] [jour 3|DAY]", {"type": "places", "what": ["resupply"],
                                                 "where": {"day": 3}}),
    ("stretches", "il y a des [murs|WHAT] [jour 2|DAY]",
     {"type": "stretches", "what": "steep", "where": {"day": 2}}),
    ("places", "[tours panoramiques|WHAT] [jour 2|DAY]",
     {"type": "places", "what": ["tower"], "where": {"day": 2}}),
    ("places", "[points de ravitaillement en eau|WHAT]", {"type": "places", "what": ["water"]}),
    ("places", "[alimentari|WHAT] [il 2° giorno|DAY]",
     {"type": "places", "what": ["resupply"], "where": {"day": 2}}),
    ("places", "tutte le [fontanelle|WHAT]", {"type": "places", "what": ["fountain"]}),
    ("places", "[posti letto|WHAT] [qui vicino|SCOPE]",
     {"type": "places", "what": ["lodging"], "where": {"scope": "here"}}),
    ("places", "[sistemazioni per la notte|WHAT]", {"type": "places", "what": ["sleep"]}),
    ("places", "[prendre une douche|WHAT] [ce midi|DAY]",
     {"type": "places", "what": ["shower"], "where": {"day": "today", "part": "middle"}}),
    ("places", "[faire réparer mon vélo|WHAT]", {"type": "places", "what": ["bike"]}),
    ("places", "[comprare cibo|WHAT]", {"type": "places", "what": ["resupply"]}),
    ("places", "dove [fare il bucato|WHAT]", {"type": "places", "what": ["laundry"]}),
    # the sentence language picks the sense of "bar" and "market"
    ("places", "[bar|WHAT] [aperti domenica|OPEN]",
     {"type": "places", "what": ["cafe"], "open": {"weekday": "sun"}}),
    ("places", "[bars|WHAT] [open on sunday|OPEN]",
     {"type": "places", "what": ["bar"], "open": {"weekday": "sun"}}),
    ("places", "[market|WHAT] [aperti il martedì|OPEN]",
     {"type": "places", "what": ["convenience"], "open": {"weekday": "tue"}}),
    # fillers
    ("places", "[campings municipaux|WHAT] [encore ouverts|OPEN]",
     {"type": "places", "what": ["campsite"], "open": {"now": True}}),
    ("places", "[bar|WHAT] [ancora aperti|OPEN]", {"type": "places", "what": ["cafe"],
                                                  "open": {"now": True}}),
    ("places", "[gare SNCF|WHAT] [sur le chemin|SCOPE]",
     {"type": "places", "what": ["train_station"], "where": {"scope": "route"}}),
    ("places", "[toilettes|WHAT] dans la [vue actuelle|SCOPE]",
     {"type": "places", "what": ["toilets"], "where": {"scope": "view"}}),
    ("places", "[valichi|WHAT] [strada facendo|SCOPE]",
     {"type": "places", "what": ["pass"], "where": {"scope": "route"}}),
    ("places", "[negozi|WHAT] [non oltre 23 km|RADIUS]",
     {"type": "places", "what": ["resupply"], "radius": km(23)}),
    ("places", "[panetterie|WHAT] [nel giro di 3 km|RADIUS]",
     {"type": "places", "what": ["bakery"], "radius": km(3)}),
    ("reroute", "refais [l'itinéraire complet|SCOPE] en [gravel|BIKE]",
     {"type": "reroute", "where": {"scope": "route"}, "bike": "gravel"}),
    # goals
    ("reroute", "rendre le [jour 6|DAY] [moins dur|GOAL]",
     {"type": "reroute", "where": {"day": 6}, "goal": "least_climbing"}),
    ("reroute", "[jour 2|DAY] [plus dur|GOAL]", {"type": "reroute", "where": {"day": 2},
                                                 "goal": "most_climbing"}),
    ("reroute", "[giorno 2|DAY] [più duro|GOAL]", {"type": "reroute", "where": {"day": 2},
                                                   "goal": "most_climbing"}),
    ("reroute", "[jour 4|DAY] [en évitant les chemins|GOAL]",
     {"type": "reroute", "where": {"day": 4}, "goal": "least_unpaved"}),
    # marks
    ("places", "[ponts|WHAT] [dans 3 heures|ALONG]",
     {"type": "places", "what": ["bridge"], "where": {"along": {"ref": "here", "at": h(3)}}}),
    ("places", "[ponti|WHAT] [tra 4 ore|ALONG]",
     {"type": "places", "what": ["bridge"], "where": {"along": {"ref": "here", "at": h(4)}}}),
    ("places", "[Campingplätze|WHAT] [Taag 3|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": 3}}),
]


@pytest.mark.parametrize("intent,markup,gold", ROUND3, ids=[c[1] for c in ROUND3])
def test_round3(intent, markup, gold):
    test_decode(intent, markup, gold)


@pytest.mark.parametrize("text,name", [
    ("l'Alpe d'Huez", "Alpe d'Huez"), ("dell'Abetone", "Abetone"),
    ("dall'Hotel Sole", "Hotel Sole"),
    ("all'Aprica", "Aprica"), ("nell'Oltrepò", "Oltrepò"), ("sull'Etna", "Etna"),
    ("qu'Annecy", "Annecy"), ("L'Aquila", "L'Aquila"),
])
def test_name_strips_elision(text, name):
    assert name_of(text) == name


# --- round 4: errors on correct model tags -----------------------------------------------------

ROUND4 = [
    # 1. typos in closed lists, from 5 letters
    ("places", "[campsites|WHAT] [tomorow evening|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": "tomorrow", "part": "end"}}),
    ("places", "[eau|WHAT] [demian|DAY]", {"type": "places", "what": ["water"],
                                          "where": {"day": "tomorrow"}}),
    ("places", "[Wasser|WHAT] auf den [letzen 20 km|ALONG]",
     {"type": "places", "what": ["water"], "where": {"along": {"ref": "end", "to": km(20)}}}),
    ("places", "[acqua|WHAT] nei [prossmi 30 km|ALONG]",
     {"type": "places", "what": ["water"], "where": {"along": {"ref": "here", "to": km(30)}}}),
    ("stretches", "[climbs|WHAT] [longr than 5 km|MIN]",
     {"type": "stretches", "what": "climb", "min": km(5)}),
    ("stretches", "[climbs|WHAT] [ovre 500 m|MIN]",
     {"type": "stretches", "what": "climb", "min": {"value": 500, "unit": "m"}}),
    ("places", "[pharmacy|WHAT] [open on sundya|OPEN]",
     {"type": "places", "what": ["pharmacy"], "open": {"weekday": "sun"}}),
    # 2. a typo as close to two values: the typed first letter decides, else ignored
    ("places", "[Apotheke|WHAT] [Sontag geöffnet|OPEN]",
     {"type": "places", "what": ["pharmacy"], "open": {"weekday": "sun"}}),
    ("places", "[shops|WHAT] [open on thuesday|OPEN]",
     {"type": "places", "what": ["resupply"], "open": {"now": True}, "ignored": ["thuesday"]}),
    # 3. a span of stop words only carries nothing
    ("places", "[campsites|WHAT] [the|NEAR] [day 3|DAY]",
     {"type": "places", "what": ["campsite"], "where": {"day": 3}}),
    ("route", "route [un|FROM] to [Lyon|TO]", {"type": "route", "to": {"name": "Lyon"}}),
    # 4. "top of" leads to a name; of several spans, the real point wins
    ("route", "take me to [the top of the Belchen|TO]",
     {"type": "route", "to": {"name": "Belchen"}}),
    ("route", "portami [in cima al Mottarone|TO]", {"type": "route", "to": {"name": "Mottarone"}}),
    ("route", "itinéraire [au sommet du Ventoux|TO]", {"type": "route", "to": {"name": "Ventoux"}}),
    ("route", "take me to [the top|TO] of [the Belchen|TO]",
     {"type": "route", "to": {"name": "Belchen"}, "ignored": ["the top"]}),
    # 5. a kind and a point kind in one point span
    ("add_point", "add [coffee stops|POINT] [every 2 hours|EVERY]",
     {"type": "add_point", "point": {"kind": "cafe"}, "kind": "stop", "every": h(2)}),
    ("add_point", "[Bäcker-Halt|POINT] an [Tag 2|DAY] einplanen",
     {"type": "add_point", "point": {"kind": "bakery"}, "kind": "stop", "where": {"day": 2}}),
    ("add_point", "ajoute des [arrêts boulangerie|POINT]",
     {"type": "add_point", "point": {"kind": "bakery"}, "kind": "stop"}),
    ("add_point", "aggiungi [soste pranzo|POINT]",
     {"type": "add_point", "point": {"kind": "food"}, "kind": "stop"}),
    ("add_point", "[Mittagspause|POINT] einplanen",
     {"type": "add_point", "point": {"kind": "food"}, "kind": "stop"}),
    ("remove_point", "remove the [coffee stop|POINT]",
     {"type": "remove_point", "point": {"kind": "cafe"}}),
    # 6. bike compounds and hyphens
    ("route", "[Gravel-Tour|BIKE] nach [Bern|TO]",
     {"type": "route", "to": {"name": "Bern"}, "bike": "gravel"}),
    ("route", "[MTB-Strecke|BIKE] zum [Feldberg|TO]",
     {"type": "route", "to": {"name": "Feldberg"}, "bike": "mtb"}),
    ("route", "[Rennradstrcke|BIKE] nach [Basel|TO]",
     {"type": "route", "to": {"name": "Basel"}, "bike": "road"}),
    # 7. split verbs in count spans
    ("split", "[in 5 Tage aufteilen|DAYS]", {"type": "split", "days": 5}),
    ("split", "[dividere in tappe da 80 km|PER_DAY]", {"type": "split", "per_day": km(80)}),
    # 8. a capitalised bare kind word in NEAR is a town
    ("places", "[fountains|WHAT] in [Essen|NEAR]",
     {"type": "places", "what": ["fountain"], "where": {"near": [{"name": "Essen"}]}}),
    ("places", "[campsites|WHAT] near [a supermarket|NEAR]",
     {"type": "places", "what": ["campsite"], "ignored": ["a supermarket"]}),
    # 9. greetings carry nothing
    ("places", "[hallo|IGNORED] [Campingplätze|WHAT] [morgen|DAY] [danke|IGNORED]",
     {"type": "places", "what": ["campsite"], "where": {"day": "tomorrow"}}),
]


@pytest.mark.parametrize("intent,markup,gold", ROUND4, ids=[c[1] for c in ROUND4])
def test_round4(intent, markup, gold):
    test_decode(intent, markup, gold)
