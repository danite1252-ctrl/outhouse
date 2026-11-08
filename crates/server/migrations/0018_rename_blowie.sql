-- The bonus fly sound is now "Blowfly" (key "fly"); "blowie" means something
-- else in Australia. Move anything already stored under the old key.
UPDATE builtin_sound_favourites SET sound_key = 'fly' WHERE sound_key = 'blowie'
    AND NOT EXISTS (SELECT 1 FROM builtin_sound_favourites f2
                    WHERE f2.user_id = builtin_sound_favourites.user_id AND f2.sound_key = 'fly');
DELETE FROM builtin_sound_favourites WHERE sound_key = 'blowie';
UPDATE hidden_builtin_sounds SET sound_key = 'fly' WHERE sound_key = 'blowie'
    AND NOT EXISTS (SELECT 1 FROM hidden_builtin_sounds h2
                    WHERE h2.guild_id = hidden_builtin_sounds.guild_id AND h2.sound_key = 'fly');
DELETE FROM hidden_builtin_sounds WHERE sound_key = 'blowie';
UPDATE entrance_sounds SET builtin = 'fly' WHERE builtin = 'blowie';
