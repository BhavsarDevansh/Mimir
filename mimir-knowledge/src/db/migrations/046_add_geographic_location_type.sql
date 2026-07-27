-- 046: add the "Geographic" location type (Phase 3 C2 / #196).
--
-- `Visited` / `Home` / `Work` / `Origin` / `Current` describe how a *person or
-- organisation* relates to a location (where they live / work / have been). A
-- `Place` entity, however, does not "visit" a location — it *is* a location.
-- The Photos connector (C2) anchors each place entity created from a photo's
-- GPS with a coordinate row typed `Geographic`, so `find_nearby` (S4) can
-- resolve "which places are near this point" by place identity rather than
-- only by where the owner has been. Additive `INSERT` into the
-- `location_types` lookup; no data changes, no table rebuild.

INSERT INTO location_types (id, name) VALUES (6, 'Geographic');
