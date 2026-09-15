# Photo CI evidence

CI run `34944416233`, board job `104300423590`, measured `7e262186`:
App 48,504 bytes; linked resident 303,832 bytes; flash 1,581,100 bytes;
residual stack 55,592 bytes; task body 4,072 bytes; boot chain 7,792 bytes.
The unchanged arena is 131,072 bytes in a 132,096-byte uninitialized section.
The 9,784-byte poll frame and recorded 37,016-byte hardware high-water are
unchanged. Margin is 18,576 bytes over that high-water, above the unchanged
8,704-byte floor. The exact App record now matches the shipping image.
No device cap, hardware record, local image, or base rebuild changed.
The parent includes current snapshot staging and Linux fixture-label fixes.
Final CI remains required.
