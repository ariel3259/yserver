// Text churn for #137 step 5: how often does the glyph path actually run,
// and therefore how often does the tier-1 one-pixel readback happen?
//
// The reporter's probe is static -- it paints its text once and sits there,
// which proves the fix but sizes nothing. This repaints a panel full of
// strings as fast as Swing will let it, in a NEW colour each frame, so
// Java2D cannot reuse a source picture and every frame goes down the
// XRSolidSrcPict route. That is the worst case for a per-draw readback.
//
// The colour also changes on purpose: a stale cache would show as the text
// lagging a frame behind the colour, and the frame counter in the corner
// gives a run's total to divide the telemetry counters by.
import java.awt.*;
import javax.swing.*;

public final class JavaTextChurn {
    private static final String[] LINES = {
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz 0123456789",
        "Prilis zlutoucky kun upel dabelske ody",
        "The quick brown fox jumps over the lazy dog.",
        "Glyphs: @ # $ % & * () [] {} <> /\\ | ~ + = ",
    };
    static int frame;

    public static void main(String[] args) {
        SwingUtilities.invokeLater(() -> {
            JFrame f = new JFrame("Java text churn");
            f.setDefaultCloseOperation(WindowConstants.EXIT_ON_CLOSE);
            JPanel p = new JPanel() {
                @Override
                protected void paintComponent(Graphics g) {
                    super.paintComponent(g);
                    Graphics2D g2 = (Graphics2D) g;
                    g2.setColor(Color.WHITE);
                    g2.fillRect(0, 0, getWidth(), getHeight());
                    g2.setFont(g2.getFont().deriveFont(Font.PLAIN, 14f));
                    int y = 24;
                    for (int rep = 0; rep < 8; rep++) {
                        // A different colour per line per frame, so no two
                        // draws can share a source picture.
                        for (String s : LINES) {
                            g2.setColor(new Color((frame * 7 + y) & 0xFF,
                                                  (frame * 13 + rep * 31) & 0xFF,
                                                  (y * 5 + rep) & 0xFF));
                            g2.drawString(s, 12, y);
                            y += 20;
                        }
                    }
                    g2.setColor(Color.BLACK);
                    g2.drawString("frame " + frame, 12, y + 10);
                }
            };
            p.setPreferredSize(new Dimension(900, 720));
            f.setContentPane(p);
            f.pack();
            f.setVisible(true);
            new Timer(10, e -> { frame++; p.repaint(); }).start();
        });
    }
}
