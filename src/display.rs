use windows::Win32::Foundation::RECT;

#[derive(Debug, Clone, Default)]
pub struct DisplayConfiguration {
    pub bounds: Rectangle,
    pub displays: Vec<Display>,
}

impl DisplayConfiguration {
    pub fn normalize(&mut self) -> &mut Self {
        for x in &mut self.displays {
            x.bounds.move_by(-self.bounds.min_x, -self.bounds.min_y);
        }
        self.bounds.normalize();
        self
    }
    
    pub fn normalized(&self) -> DisplayConfiguration {
        let mut clone = self.clone();
        clone.normalize();
        clone
    }
    
    pub fn show_displays(&self) {
        println!("Detected displays ({} total):", self.displays.len());
        for (i, display) in self.displays.iter().enumerate() {
            let (width, height) = display.bounds.resolution();
            println!("{}. {} ({}x{})", i + 1, display.name, width, height);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Display {
    pub name: String,
    pub bounds: Rectangle,
}

#[derive(Debug, Clone, Default)]
pub struct Rectangle {
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
}

impl Rectangle {
    pub fn resolution(&self) -> (u32, u32) {
        ((self.max_x - self.min_x) as u32, (self.max_y - self.min_y) as u32)
    }
    
    pub fn normalize(&mut self) -> &mut Self {
        self.max_x -= self.min_x;
        self.max_y -= self.min_y;
        self.min_x = 0;
        self.min_y = 0;
        self
    }
    
    pub fn normalized(&self) -> Rectangle {
        let mut clone = self.clone();
        clone.normalize();
        clone
    }
    
    pub fn move_by(&mut self, x: i32, y: i32) -> &mut Self {
        self.min_x += x;
        self.max_x += x;
        self.min_y += y;
        self.max_y += y;
        self
    }
    
    pub fn moved_by(&self, x: i32, y: i32) -> Rectangle {
        let mut clone = self.clone();
        clone.move_by(x, y);
        clone
    }
}

impl From<RECT> for Rectangle {
    fn from(value: RECT) -> Self {
        Rectangle {
            min_x: value.left,
            max_x: value.right,
            min_y: value.top,
            max_y: value.bottom,
        }
    }
}