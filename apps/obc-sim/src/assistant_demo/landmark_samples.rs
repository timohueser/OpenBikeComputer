//! Fixed random category samples. Source revisions and draw seed are recorded with the captures.

use obc_app::assistant_demo::{photos::Photo, Landmark};

static PHOTO_Q109647737: Photo = Photo {
    pixels: include_bytes!("../../assets/landmarks/Q109647737.rgb222"),
    credit: "Bgvr. Resized and ordered dither. CC BY-SA 4.0.",
    source: "https://commons.wikimedia.org/wiki/File:Gutzgletscher_20160902.jpg",
    licence: "https://creativecommons.org/licenses/by-sa/4.0/",
};

static Q109647737: Landmark = Landmark {
    kind: "Glacier",
    article: "https://de.wikipedia.org/wiki/Gutzgletscher",
    photo: Some(&PHOTO_Q109647737),
    pages: &[
        "A hanging glacier on the Wetterhorn's north face above Grindelwald.",
        "Large icefalls can plunge over 1,000m. Its meltwater feeds the Briggbach.",
    ],
};

static PHOTO_Q676713: Photo = Photo {
    pixels: include_bytes!("../../assets/landmarks/Q676713.rgb222"),
    credit: "Zacharie Grossen. Resized and ordered dither. CC BY-SA 4.0.",
    source: "https://commons.wikimedia.org/wiki/File:Summer_Snow.jpg",
    licence: "https://creativecommons.org/licenses/by-sa/4.0/",
};

static Q676713: Landmark = Landmark {
    kind: "Glacier",
    article: "https://en.wikipedia.org/wiki/Tsanfleuron_Glacier",
    photo: Some(&PHOTO_Q676713),
    pages: &[
        "Tsanfleuron lies in the western Bernese Alps. Its length was measured at 3.5km in 2005.",
        "Much of the glacier is used by the Glacier 3000 ski area, below the Scex Rouge and Oldenhorn.",
    ],
};

static PHOTO_Q675092: Photo = Photo {
    pixels: include_bytes!("../../assets/landmarks/Q675092.rgb222"),
    credit: "(c) Hans Hillewaert. Resized and ordered dither. CC BY-SA 3.0.",
    source: "https://commons.wikimedia.org/wiki/File:Langgletscher_in_L%C3%B6tschental.jpg",
    licence: "https://creativecommons.org/licenses/by-sa/3.0/",
};

static Q675092: Landmark = Landmark {
    kind: "Glacier",
    article: "https://en.wikipedia.org/wiki/Lang_Glacier",
    photo: Some(&PHOTO_Q675092),
    pages: &[
        "A glacier in the Bernese Alps in Valais. In 2005 it was measured at 6.6km long.",
        "Its recorded area was 10.1 square km in 1973.",
    ],
};

pub const GLACIERS: [(&str, &Landmark); 3] =
    [("Gutzgletscher", &Q109647737), ("Tsanfleuron", &Q676713), ("Lang Glacier", &Q675092)];

static PHOTO_Q667398: Photo = Photo {
    pixels: include_bytes!("../../assets/landmarks/Q667398.rgb222"),
    credit: "Roland Zumbuehl. Resized and ordered dither. CC BY-SA 3.0.",
    source: "https://commons.wikimedia.org/wiki/File:Klausenpass_Hotel_Passhoehe.jpg",
    licence: "http://creativecommons.org/licenses/by-sa/3.0/",
};

static Q667398: Landmark = Landmark {
    kind: "Pass",
    article: "https://en.wikipedia.org/wiki/Klausen_Pass",
    photo: Some(&PHOTO_Q667398),
    pages: &[
        "At 1,948m, Klausen Pass connects Altdorf in Uri with Linthal in Glarus.",
        "The canton boundary lies about 8km down the Linthal side, rather than at the summit.",
    ],
};

static Q7909167: Landmark = Landmark {
    kind: "Pass",
    article: "https://en.wikipedia.org/wiki/Val_Viola_Pass",
    photo: None,
    pages: &[
        "A 2,468m pass on the Swiss-Italian border, between Piz Val Nera and Corno di Dosde.",
        "A trail crosses the pass, linking Poschiavo in Graubunden with Valdidentro in Lombardy.",
    ],
};

static Q3897361: Landmark = Landmark {
    kind: "Pass",
    article: "https://en.wikipedia.org/wiki/Campolungo_Pass",
    photo: None,
    pages: &[
        "At 2,318m, Campolungo is the lowest pass between the Maggia and Leventina valleys in Ticino.",
        "It connects Fusio and Prat, between Pizzo Massari to the north and Pizzo Campolungo to the south.",
    ],
};

pub const PASSES: [(&str, &Landmark); 3] =
    [("Klausen Pass", &Q667398), ("Val Viola Pass", &Q7909167), ("Campolungo Pass", &Q3897361)];
