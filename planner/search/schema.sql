CREATE TABLE place_contexts(id INTEGER PRIMARY KEY,city TEXT,postcode TEXT,region TEXT,context TEXT,country TEXT);
CREATE TABLE place_records(id INTEGER PRIMARY KEY,source TEXT,name TEXT,aliases TEXT,
 kind TEXT,lon REAL,lat REAL,context_id INTEGER,importance REAL,
 west REAL,south REAL,east REAL,north REAL,cuisine TEXT,opening_hours TEXT,
 website TEXT,phone TEXT,description TEXT);
CREATE TABLE place_content(place_id INTEGER PRIMARY KEY,content TEXT NOT NULL,landmark_id TEXT);
CREATE TABLE place_identities(place_id INTEGER,identity TEXT,PRIMARY KEY(place_id,identity)) WITHOUT ROWID;
CREATE INDEX identity_places ON place_identities(identity);
CREATE VIEW places AS SELECT p.id,p.source,p.name,COALESCE(p.aliases,p.name) AS aliases,
 p.kind,p.lon,p.lat,c.city,c.postcode,p.importance,p.west,p.south,p.east,p.north,
 c.region,c.context,c.country,p.cuisine,p.opening_hours,p.website,p.phone,p.description,article.content,article.landmark_id
 FROM place_records p JOIN place_contexts c ON c.id=p.context_id LEFT JOIN place_content article ON article.place_id=p.id;
CREATE TABLE addresses(street_id INTEGER,house TEXT,lon REAL,lat REAL,source TEXT);
CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT);
